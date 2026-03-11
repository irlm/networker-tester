#Requires -Version 5.1
# ──────────────────────────────────────────────────────────────────────────────
# Networker Tester – Windows interactive installer (rustup-style)
#
# Installs networker-tester and/or networker-endpoint either:
#   locally  – on this machine (release binary download or source compile)
#   remotely – provisioned on a cloud VM (Azure, AWS, and GCP supported)
#
# Two local install modes (auto-detected, or choose in customize flow):
#   release  – download pre-built binary from the latest GitHub release via
#              gh CLI (fast, ~10 s); requires: gh installed + gh auth login
#   source   – compile from source via cargo install (slower, ~5-10 min);
#              requires: Rust/cargo  (repo is public – no SSH key needed)
#
# Usage (piped):
#   irm <raw-gist-url>/install.ps1 | iex
#
# Usage (downloaded):
#   .\install.ps1 [-Component tester|endpoint|both] [-Yes] [-FromSource]
#                 [-SkipRust] [-Azure] [-TesterAzure] [-Aws] [-TesterAws]
#                 [-Gcp] [-TesterGcp] [-Help]
# ──────────────────────────────────────────────────────────────────────────────

# PSScriptAnalyzer suppressions — interactive installer uses Write-Host for
# colored output, plural nouns for clarity, params consumed via $script: scope.
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '')]
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSReviewUnusedParameter', '')]
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSUseSingularNouns', '')]
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSUseBOMForUnicodeEncodedFile', '')]
param(
    [string]$Component  = "",
    [switch]$Yes,
    [switch]$FromSource,
    [switch]$SkipRust,
    [switch]$Azure,
    [switch]$TesterAzure,
    [switch]$Aws,
    [switch]$TesterAws,
    [switch]$Gcp,
    [switch]$TesterGcp,
    [string]$Region     = "",
    [string]$AwsRegion  = "",
    [string]$GcpProject = "",
    [string]$GcpZone    = "",
    [switch]$Help
)

$ErrorActionPreference = "Stop"

$RepoHttps     = "https://github.com/irlm/networker-tester"
$RepoGh        = "irlm/networker-tester"
$CargoBin      = Join-Path $env:USERPROFILE ".cargo\bin"
$InstallerVersion = "v0.13.7"  # fallback when gh is unavailable

# ── Print helpers ──────────────────────────────────────────────────────────────
function Write-Ok   ($msg) { Write-Host "  v " -NoNewline -ForegroundColor Green;   Write-Host $msg }
function Write-Warn ($msg) { Write-Host "  ! " -NoNewline -ForegroundColor Yellow;  Write-Host $msg }
function Write-Err  ($msg) { Write-Host "  x $msg" -ForegroundColor Red }
function Write-Info ($msg) { Write-Host "  > " -NoNewline -ForegroundColor Cyan;    Write-Host $msg }
function Write-Dim  ($msg) { Write-Host "    $msg" -ForegroundColor DarkGray }

function Write-Banner {
    Write-Host ""
    Write-Host ("=" * 58) -ForegroundColor Cyan
    if ($script:NetworkerVersion) {
        Write-Host ("      Networker Tester  " + $script:NetworkerVersion) -ForegroundColor Cyan
    } else {
        Write-Host ("      Networker Tester Installer") -ForegroundColor Cyan
    }
    Write-Host ("=" * 58) -ForegroundColor Cyan
    Write-Host ""
}

function Write-Section ($title) {
    Write-Host ""
    Write-Host "---- $title ----" -ForegroundColor White
}

function Write-StepHeader ($n, $title) {
    Write-Host ""
    Write-Host ("Step " + $n + ": " + $title) -ForegroundColor White
}

function Show-Help {
    Write-Host "Usage: install.ps1 [-Component tester|endpoint|both] [options]"
    Write-Host ""
    Write-Host "  -Component   tester    Install networker-tester  [default: both]"
    Write-Host "               endpoint  Install networker-endpoint"
    Write-Host "               both      Install both binaries"
    Write-Host ""
    Write-Host "Install modes (auto-detected; override in customize flow or via flag):"
    Write-Host "  release   Download pre-built binary via gh CLI -- fast (~10 s)"
    Write-Host "            Requires: gh installed and authenticated (gh auth login)"
    Write-Host "  source    Compile from source via cargo install -- slower (~5-10 min)"
    Write-Host "            Repo is public -- no SSH key required"
    Write-Host ""
    Write-Host "Cloud deployment (deploy to remote VM):"
    Write-Host "  -Azure           Deploy endpoint to Azure VM"
    Write-Host "  -TesterAzure     Deploy tester to Azure VM"
    Write-Host "  -Aws             Deploy endpoint to AWS EC2"
    Write-Host "  -TesterAws       Deploy tester to AWS EC2"
    Write-Host "  -Gcp             Deploy endpoint to GCP GCE"
    Write-Host "  -TesterGcp       Deploy tester to GCP GCE"
    Write-Host "  -Region REGION   Azure region (default: eastus)"
    Write-Host "  -AwsRegion REG   AWS region (default: us-east-1)"
    Write-Host "  -GcpProject ID   GCP project ID"
    Write-Host "  -GcpZone ZONE    GCP zone (default: us-central1-a)"
    Write-Host ""
    Write-Host "  -Yes           Non-interactive: accept all defaults"
    Write-Host "  -FromSource    Force source-compile mode (skip release detection)"
    Write-Host "  -SkipRust      Skip Rust installation (source mode)"
    Write-Host "  -Help          Show this help message"
    Write-Host ""
    Write-Host "Examples:"
    Write-Host "  .\install.ps1 -Component tester"
    Write-Host "  .\install.ps1 -Yes -Component endpoint"
    Write-Host "  .\install.ps1 -Azure -Component endpoint"
    Write-Host "  .\install.ps1 -TesterAws -Aws"
}

# ── Script-level state ────────────────────────────────────────────────────────
$script:InstallMethod     = "source"   # "release" | "source"
$script:ReleaseAvailable  = $false
$script:ReleaseTarget     = ""
$script:NetworkerVersion  = ""
$script:DoRustInstall     = $false
$script:DoInstallTester   = $true
$script:DoInstallEndpoint = $true
$script:RustExists        = $false
$script:RustVer           = "not installed"
$script:GitAvailable      = $false
$script:WingetAvailable   = $false
$script:DoGitInstall      = $false
$script:MsvcAvailable     = $true
$script:DoMsvcInstall     = $false
$script:ChromeAvailable   = $false
$script:ChromePath        = ""
$script:DoChromiumInstall  = $false
$script:SysOs             = ""
$script:SysArch           = ""
$script:StepNum           = 0

# ── Remote deployment state ───────────────────────────────────────────────────
$script:TesterLocation    = "local"    # "local" | "azure" | "aws" | "gcp" | "lan"
$script:EndpointLocation  = "local"    # "local" | "azure" | "aws" | "gcp" | "lan"
$script:DoRemoteTester    = $false
$script:DoRemoteEndpoint  = $false

# ── LAN state ────────────────────────────────────────────────────────────────
$script:LanTesterIp       = ""
$script:LanTesterUser     = ""
$script:LanTesterPort     = "22"
$script:LanTesterOs       = ""

$script:LanEndpointIp     = ""
$script:LanEndpointUser   = ""
$script:LanEndpointPort   = "22"
$script:LanEndpointOs     = ""

# ── Azure state ──────────────────────────────────────────────────────────────
$script:AzureCliAvailable = $false
$script:AzureLoggedIn     = $false
$script:AzureRegion       = "eastus"
$script:AzureRegionAsked  = $false
$script:AzureTesterRg     = "networker-rg-tester"
$script:AzureTesterVm     = "networker-tester-vm"
$script:AzureTesterSize   = "Standard_B2s"
$script:AzureTesterOs     = "linux"
$script:AzureTesterIp     = ""
$script:AzureEndpointRg   = "networker-rg-endpoint"
$script:AzureEndpointVm   = "networker-endpoint-vm"
$script:AzureEndpointSize = "Standard_B2s"
$script:AzureEndpointOs   = "linux"
$script:AzureEndpointIp   = ""
$script:AzureAutoShutdown = "yes"
$script:AzureShutdownAsked = $false

# ── AWS state ────────────────────────────────────────────────────────────────
$script:AwsCliAvailable   = $false
$script:AwsLoggedIn       = $false
$script:AwsRegion         = "us-east-1"
$script:AwsRegionAsked    = $false
$script:AwsTesterName     = "networker-tester"
$script:AwsTesterType     = "t3.small"
$script:AwsTesterOs       = "linux"
$script:AwsTesterInstanceId = ""
$script:AwsTesterIp       = ""
$script:AwsEndpointName   = "networker-endpoint"
$script:AwsEndpointType   = "t3.small"
$script:AwsEndpointOs     = "linux"
$script:AwsEndpointInstanceId = ""
$script:AwsEndpointIp     = ""
$script:AwsAutoShutdown   = "yes"
$script:AwsShutdownAsked  = $false
$script:AwsAmiId          = ""

# ── GCP state ────────────────────────────────────────────────────────────────
$script:GcpCliAvailable   = $false
$script:GcpLoggedIn       = $false
$script:GcpProject        = ""
$script:GcpRegion         = "us-central1"
$script:GcpZone           = "us-central1-a"
$script:GcpRegionAsked    = $false
$script:GcpTesterName     = "networker-tester"
$script:GcpTesterMachineType = "e2-small"
$script:GcpTesterIp       = ""
$script:GcpEndpointName   = "networker-endpoint"
$script:GcpEndpointMachineType = "e2-small"
$script:GcpEndpointIp     = ""
$script:GcpTesterOs       = "linux"
$script:GcpEndpointOs     = "linux"
$script:GcpAutoShutdown   = "yes"
$script:GcpShutdownAsked  = $false

$script:ConfigFilePath    = ""

# ── Target triple detection ────────────────────────────────────────────────────
function Get-ReleaseTarget {
    switch ($env:PROCESSOR_ARCHITECTURE) {
        "AMD64"  { return "x86_64-pc-windows-msvc" }
        default  { return "" }   # ARM64/x86 not yet in release matrix
    }
}

# ── Chrome/Chromium detection ──────────────────────────────────────────────────
function Get-ChromePath {
    if ($env:NETWORKER_CHROME_PATH -and (Test-Path $env:NETWORKER_CHROME_PATH)) {
        return $env:NETWORKER_CHROME_PATH
    }
    $paths = @(
        "${env:ProgramFiles}\Google\Chrome\Application\chrome.exe",
        "${env:LocalAppData}\Google\Chrome\Application\chrome.exe",
        "${env:ProgramFiles(x86)}\Google\Chrome\Application\chrome.exe",
        "${env:ProgramFiles}\Chromium\Application\chrome.exe"
    )
    foreach ($p in $paths) {
        if (Test-Path $p) { return $p }
    }
    return $null
}

# ── Yes/No helper ─────────────────────────────────────────────────────────────
function Invoke-AskYN ($prompt, $default) {
    if ($Yes) {
        return ($default -eq "y")
    }
    while ($true) {
        if ($default -eq "y") {
            $ans = Read-Host "  $prompt [Y/n]"
        } else {
            $ans = Read-Host "  $prompt [y/N]"
        }
        if ([string]::IsNullOrWhiteSpace($ans)) { $ans = $default }
        switch ($ans.Trim().ToLower()) {
            "y"   { return $true  }
            "yes" { return $true  }
            "n"   { return $false }
            "no"  { return $false }
            default { Write-Warn "Please enter y or n." }
        }
    }
}

# ── Read-Host with default ────────────────────────────────────────────────────
function Read-HostDefault ($prompt, $default) {
    if ($Yes) { return $default }
    $ans = Read-Host $prompt
    if ([string]::IsNullOrWhiteSpace($ans)) { return $default }
    return $ans.Trim()
}

# ── System discovery ───────────────────────────────────────────────────────────
function Invoke-DiscoverSystem {
    $script:SysOs   = [System.Environment]::OSVersion.VersionString
    $script:SysArch = $env:PROCESSOR_ARCHITECTURE

    $cargoCmd = Get-Command cargo -ErrorAction SilentlyContinue
    if ($cargoCmd) {
        $script:RustExists = $true
        $script:RustVer    = (& rustc --version 2>&1)
    } else {
        $script:RustExists = $false
        $script:RustVer    = "not installed"
    }

    if (-not $script:RustExists -and -not $SkipRust) { $script:DoRustInstall = $true }

    # Git + winget detection
    $script:GitAvailable    = $null -ne (Get-Command git    -ErrorAction SilentlyContinue)
    $script:WingetAvailable = $null -ne (Get-Command winget -ErrorAction SilentlyContinue)

    # MSVC C++ Build Tools detection
    $vswhereExe = Join-Path ([System.Environment]::GetFolderPath('ProgramFilesX86')) `
                             "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhereExe) {
        $vsPath = & $vswhereExe -latest -products * `
            -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
            -property installationPath 2>&1
        $script:MsvcAvailable = -not [string]::IsNullOrWhiteSpace($vsPath)
    } else {
        $script:MsvcAvailable = $null -ne (Get-Command link -ErrorAction SilentlyContinue)
    }

    switch ($Component) {
        "tester"   { $script:DoInstallEndpoint = $false }
        "endpoint" { $script:DoInstallTester   = $false }
    }

    # Release mode: available when gh is authenticated AND platform is in release matrix
    if (-not $FromSource) {
        $target = Get-ReleaseTarget
        $ghCmd  = Get-Command gh -ErrorAction SilentlyContinue
        if ($ghCmd -and $target) {
            $prevErr = $ErrorActionPreference
            $ErrorActionPreference = "Continue"
            $null = & gh auth status 2>&1
            $ghOk = ($LASTEXITCODE -eq 0)
            $ErrorActionPreference = $prevErr
            if ($ghOk) {
                $script:ReleaseTarget    = $target
                $script:ReleaseAvailable = $true
                $script:InstallMethod    = "release"
                $script:NetworkerVersion = (& gh release list --repo $RepoGh `
                    --limit 1 --json tagName --jq ".[0].tagName" 2>$null) -join ""
            }
        }
    }

    # Fallback version if gh not available
    if (-not $script:NetworkerVersion) {
        $script:NetworkerVersion = $InstallerVersion
    }

    # Auto-offer git install only in source mode
    if ($script:InstallMethod -eq "source" -and -not $script:GitAvailable -and $script:WingetAvailable) {
        $script:DoGitInstall = $true
    }

    # Auto-offer MSVC install only in source mode
    if ($script:InstallMethod -eq "source" -and -not $script:MsvcAvailable -and $script:WingetAvailable) {
        $script:DoMsvcInstall = $true
    }

    # Chrome detection
    $script:ChromePath      = Get-ChromePath
    $script:ChromeAvailable = -not [string]::IsNullOrWhiteSpace($script:ChromePath)

    # ── Cloud CLI detection ──────────────────────────────────────────────────
    $script:AzureCliAvailable = $null -ne (Get-Command az -ErrorAction SilentlyContinue)
    if ($script:AzureCliAvailable) {
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $null = & az account show --output none 2>&1
        $script:AzureLoggedIn = ($LASTEXITCODE -eq 0)
        $ErrorActionPreference = $prevErr
    }

    $script:AwsCliAvailable = $null -ne (Get-Command aws -ErrorAction SilentlyContinue)
    if ($script:AwsCliAvailable) {
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $null = & aws sts get-caller-identity 2>&1
        $script:AwsLoggedIn = ($LASTEXITCODE -eq 0)
        $ErrorActionPreference = $prevErr
    }

    $script:GcpCliAvailable = $null -ne (Get-Command gcloud -ErrorAction SilentlyContinue)
    if ($script:GcpCliAvailable) {
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $acct = (& gcloud config get-value account 2>$null) -join ""
        if ($acct -and $acct -ne "(unset)") {
            $script:GcpLoggedIn = $true
        }
        $ErrorActionPreference = $prevErr
    }

    # Handle CLI flags for remote deployment
    if ($Azure)       { $script:EndpointLocation = "azure"; $script:DoRemoteEndpoint = $true }
    if ($TesterAzure) { $script:TesterLocation   = "azure"; $script:DoRemoteTester   = $true }
    if ($Aws)         { $script:EndpointLocation = "aws";   $script:DoRemoteEndpoint = $true }
    if ($TesterAws)   { $script:TesterLocation   = "aws";   $script:DoRemoteTester   = $true }
    if ($Gcp)         { $script:EndpointLocation = "gcp";   $script:DoRemoteEndpoint = $true }
    if ($TesterGcp)   { $script:TesterLocation   = "gcp";   $script:DoRemoteTester   = $true }
    if ($Region)      { $script:AzureRegion = $Region }
    if ($AwsRegion)   { $script:AwsRegion   = $AwsRegion }
    if ($GcpProject)  { $script:GcpProject  = $GcpProject }
    if ($GcpZone)     { $script:GcpZone     = $GcpZone; $script:GcpRegion = $GcpZone -replace '-[a-z]$','' }
}

# ── Display helpers ────────────────────────────────────────────────────────────
function Show-SystemInfo {
    Write-Section "System Information"
    Write-Host ""
    Write-Host ("    {0,-22} {1}" -f "OS:",           $script:SysOs)
    Write-Host ("    {0,-22} {1}" -f "Architecture:", $script:SysArch)
    Write-Host ("    {0,-22} {1}" -f "User home:",    $env:USERPROFILE)
    Write-Host ("    {0,-22} {1}" -f "Rust / cargo:", $script:RustVer)
    if ($script:GitAvailable) {
        $gitVer = (& git --version 2>&1)
        Write-Host ("    {0,-22} {1}" -f "git:", $gitVer)
    } else {
        Write-Host ("    {0,-22} {1}" -f "git:", "not installed")
    }
    if ($script:MsvcAvailable) {
        Write-Host ("    {0,-22} {1}" -f "VC++ build tools:", "installed v")
    } else {
        Write-Host ("    {0,-22} {1}" -f "VC++ build tools:", "not installed")
    }
    if ($script:ChromeAvailable) {
        Write-Host ("    {0,-22} {1}" -f "Chrome/Chromium:", "installed v")
    } else {
        Write-Host ("    {0,-22} {1}" -f "Chrome/Chromium:", "not installed  (browser probe disabled)")
    }
    Write-Host ("    {0,-22} {1}" -f "Install to:",   $CargoBin)
    if ($script:ReleaseAvailable) {
        Write-Host ("    {0,-22} {1}" -f "gh CLI:", "authenticated v")
    }
    if ($script:AzureCliAvailable) {
        $azLabel = if ($script:AzureLoggedIn) { "authenticated v" } else { "installed  (run: az login)" }
        Write-Host ("    {0,-22} {1}" -f "Azure CLI:", $azLabel)
    }
    if ($script:AwsCliAvailable) {
        $awsLabel = if ($script:AwsLoggedIn) { "authenticated v" } else { "installed  (run: aws configure)" }
        Write-Host ("    {0,-22} {1}" -f "AWS CLI:", $awsLabel)
    }
    if ($script:GcpCliAvailable) {
        $gcpLabel = if ($script:GcpLoggedIn) { "authenticated v" } else { "installed  (run: gcloud auth login)" }
        Write-Host ("    {0,-22} {1}" -f "GCP CLI:", $gcpLabel)
    }
}

function Show-Plan {
    Write-Section "Installation Plan"
    Write-Host ""
    $step = 1

    # Show local install plan
    $doLocalTester   = $script:DoInstallTester   -and -not $script:DoRemoteTester
    $doLocalEndpoint = $script:DoInstallEndpoint -and -not $script:DoRemoteEndpoint

    if ($doLocalTester -or $doLocalEndpoint) {
        if ($script:InstallMethod -eq "release") {
            Write-Host "    Method:  Download binary from GitHub release  (fast)" -ForegroundColor White
            Write-Host ("    Target:  " + $script:ReleaseTarget) -ForegroundColor DarkGray
            Write-Host ""
            $verLabel = if ($script:NetworkerVersion) { $script:NetworkerVersion } else { "latest" }
            if ($doLocalTester) {
                Write-Host ("    {0}. Download networker-tester    {1}" -f $step, $verLabel)
                $step++
            }
            if ($doLocalEndpoint) {
                Write-Host ("    {0}. Download networker-endpoint  {1}" -f $step, $verLabel)
                $step++
            }
            Write-Host ""
            $releaseLabel = if ($script:NetworkerVersion) { $script:NetworkerVersion } else { "latest release" }
            Write-Dim "Repository:  $RepoGh  ($releaseLabel)"
        } else {
            Write-Host "    Method:  Compile from source  (~5-10 min)" -ForegroundColor White
            Write-Host ""
            if (-not $script:GitAvailable) {
                if ($script:DoGitInstall) {
                    Write-Host ("    {0}. Install git            Install via winget" -f $step); $step++
                }
            }
            if (-not $script:ChromeAvailable -and $doLocalTester) {
                if ($script:DoChromiumInstall) {
                    Write-Host ("    {0}. Install Chrome         winget install Google.Chrome" -f $step); $step++
                }
            }
            if ($script:DoRustInstall) {
                Write-Host ("    {0}. Install Rust           Download rustup-init.exe" -f $step); $step++
            }
            if (-not $script:MsvcAvailable -and $script:DoMsvcInstall) {
                Write-Host ("    {0}. Install VC++ Build Tools  winget install" -f $step); $step++
            }
            $browserNote = if ($script:ChromeAvailable -or $script:DoChromiumInstall) { "  [+browser feature]" } else { "" }
            if ($doLocalTester) {
                Write-Host ("    {0}. Install networker-tester    cargo install from GitHub{1}" -f $step, $browserNote); $step++
            }
            if ($doLocalEndpoint) {
                Write-Host ("    {0}. Install networker-endpoint  cargo install from GitHub" -f $step); $step++
            }
            Write-Host ""
            Write-Dim "Repository:  $RepoHttps"
            Write-Dim "Source code is compiled locally -- no pre-built binaries are downloaded."
        }
    }

    # Show remote deployment plan
    if ($script:DoRemoteTester) {
        Write-Host ""
        $provider = $script:TesterLocation.ToUpper()
        Write-Host ("    {0}. Deploy networker-tester to {1} VM" -f $step, $provider); $step++
    }
    if ($script:DoRemoteEndpoint) {
        Write-Host ""
        $provider = $script:EndpointLocation.ToUpper()
        Write-Host ("    {0}. Deploy networker-endpoint to {1} VM" -f $step, $provider); $step++
    }
}

# ── Component selection prompt ─────────────────────────────────────────────────
function Invoke-ComponentSelection {
    if ($Yes) { return }
    if ($Component) { return }

    Write-Section "What do you want to install?"
    Write-Host ""
    Write-Host "  1) Both  -- networker-tester (client) + networker-endpoint (server)  [default]"
    Write-Host "  2) tester only   -- the diagnostic CLI for measuring HTTP/1.1, H2, H3, QUIC"
    Write-Host "  3) endpoint only -- the lightweight HTTP/QUIC test server"
    Write-Host ""

    $ans = Read-HostDefault "  Choice [1]" "1"
    switch ($ans) {
        "2" { $script:DoInstallTester = $true;  $script:DoInstallEndpoint = $false
              Write-Ok "Installing: networker-tester only" }
        "3" { $script:DoInstallTester = $false; $script:DoInstallEndpoint = $true
              Write-Ok "Installing: networker-endpoint only" }
        default { $script:DoInstallTester = $true; $script:DoInstallEndpoint = $true
                  Write-Ok "Installing: networker-tester + networker-endpoint" }
    }
    Write-Host ""
}

# ── Where-to-install prompts ──────────────────────────────────────────────────
function Invoke-DeploymentLocationPrompt {
    if ($Yes) { return }

    # Tester location
    if ($script:DoInstallTester -and -not $script:DoRemoteTester) {
        Write-Host ""
        Write-Host "  Where to install networker-tester?" -ForegroundColor White
        Write-Host "    1) Locally on this machine  [default]"
        Write-Host "    2) Remote: LAN / existing machine (SSH)"
        Write-Host "    3) Remote: Azure VM"
        Write-Host "    4) Remote: AWS EC2"
        Write-Host "    5) Remote: Google Cloud GCE"
        Write-Host ""
        $ans = Read-HostDefault "  Choice [1]" "1"
        switch ($ans) {
            "2" { $script:TesterLocation = "lan";   $script:DoRemoteTester = $true }
            "3" { $script:TesterLocation = "azure"; $script:DoRemoteTester = $true }
            "4" { $script:TesterLocation = "aws";   $script:DoRemoteTester = $true }
            "5" { $script:TesterLocation = "gcp";   $script:DoRemoteTester = $true }
        }
        if ($script:DoRemoteTester) {
            switch ($script:TesterLocation) {
                "lan"   { Invoke-LanOptions "tester" }
                "azure" { Invoke-EnsureAzureCli; Invoke-AzureOptions "tester" }
                "aws"   { Invoke-EnsureAwsCli;   Invoke-AwsOptions   "tester" }
                "gcp"   { Invoke-EnsureGcpCli;   Invoke-GcpOptions   "tester" }
            }
        }
    }

    # Endpoint location
    if ($script:DoInstallEndpoint -and -not $script:DoRemoteEndpoint) {
        Write-Host ""
        Write-Host "  Where to install networker-endpoint?" -ForegroundColor White
        Write-Host "    1) Locally on this machine  [default]"
        Write-Host "    2) Remote: LAN / existing machine (SSH)"
        Write-Host "    3) Remote: Azure VM"
        Write-Host "    4) Remote: AWS EC2"
        Write-Host "    5) Remote: Google Cloud GCE"
        Write-Host ""
        $ans = Read-HostDefault "  Choice [1]" "1"
        switch ($ans) {
            "2" { $script:EndpointLocation = "lan";   $script:DoRemoteEndpoint = $true }
            "3" { $script:EndpointLocation = "azure"; $script:DoRemoteEndpoint = $true }
            "4" { $script:EndpointLocation = "aws";   $script:DoRemoteEndpoint = $true }
            "5" { $script:EndpointLocation = "gcp";   $script:DoRemoteEndpoint = $true }
        }
        if ($script:DoRemoteEndpoint) {
            switch ($script:EndpointLocation) {
                "lan"   { Invoke-LanOptions "endpoint" }
                "azure" { Invoke-EnsureAzureCli; Invoke-AzureOptions "endpoint" }
                "aws"   { Invoke-EnsureAwsCli;   Invoke-AwsOptions   "endpoint" }
                "gcp"   { Invoke-EnsureGcpCli;   Invoke-GcpOptions   "endpoint" }
            }
        }
    }
}

# ── Main interactive prompt ────────────────────────────────────────────────────
function Invoke-MainPrompt {
    if ($Yes) { return }

    Write-Host ""
    Write-Host "Proceed with installation?" -ForegroundColor White
    Write-Host ""
    Write-Host "  1) Proceed with default installation"
    Write-Host "  2) Customize installation steps"
    Write-Host "  3) Cancel"
    Write-Host ""

    while ($true) {
        $ans = Read-Host "Enter choice [1]"
        if ([string]::IsNullOrWhiteSpace($ans)) { $ans = "1" }
        switch ($ans.Trim()) {
            "1" {
                # Ask about Chrome if not already available (source mode, winget present)
                if (-not $script:ChromeAvailable -and $script:InstallMethod -eq "source" -and $script:WingetAvailable -and $script:DoInstallTester -and -not $script:DoRemoteTester) {
                    Write-Host ""
                    $script:DoChromiumInstall = Invoke-AskYN "Chrome/Chromium not found -- install it to enable the browser probe?" "y"
                    if (-not $script:DoChromiumInstall) {
                        Write-Info "Skipping Chrome -- browser probe will be disabled."
                    }
                }
                Invoke-DeploymentLocationPrompt
                return
            }
            "2" { Invoke-CustomizeFlow; return }
            "3" { Write-Host ""; Write-Host "Installation cancelled."; exit 0 }
            default { Write-Warn "Please enter 1, 2, or 3." }
        }
    }
}

# ── Customize flow ─────────────────────────────────────────────────────────────
function Invoke-CustomizeFlow {
    Write-Section "Customize Installation"
    Write-Host ""

    if ($script:ReleaseAvailable) {
        Write-Host "  Install method:"
        Write-Host "    1) Download binary from latest release  (fast, recommended)"
        Write-Host "    2) Compile from source  (requires Rust)"
        Write-Host ""
        $methodAns = Read-HostDefault "  Choice [1]" "1"
        switch ($methodAns) {
            "2"     { $script:InstallMethod = "source"  }
            default { $script:InstallMethod = "release" }
        }
        Write-Host ""
    }

    if ($script:InstallMethod -eq "source") {
        if (-not $script:GitAvailable -and $script:WingetAvailable) {
            $script:DoGitInstall = Invoke-AskYN "git is not installed -- install it via winget?" "y"
            Write-Host ""
        }
        if (-not $script:ChromeAvailable -and $script:DoInstallTester) {
            if ($script:WingetAvailable) {
                $script:DoChromiumInstall = Invoke-AskYN "Chrome/Chromium not found -- install it to enable the browser probe?" "y"
                Write-Host ""
            }
        }
        if (-not $script:RustExists) {
            $script:DoRustInstall = Invoke-AskYN "Install Rust via rustup (win.rustup.rs)?" "y"
            Write-Host ""
        }
        if (-not $script:MsvcAvailable -and $script:WingetAvailable) {
            $script:DoMsvcInstall = Invoke-AskYN "VC++ Build Tools not found -- install via winget?" "y"
            Write-Host ""
        }
    }

    Write-Host "  Which components do you want to install?"
    Write-Host ""
    Write-Host "    1) Both  (networker-tester + networker-endpoint)  [default]"
    Write-Host "    2) tester only   -- the diagnostic CLI client"
    Write-Host "    3) endpoint only -- the target test server"
    Write-Host ""

    $compAns = Read-HostDefault "  Choice [1]" "1"
    switch ($compAns) {
        "2" { $script:DoInstallTester = $true;  $script:DoInstallEndpoint = $false }
        "3" { $script:DoInstallTester = $false; $script:DoInstallEndpoint = $true  }
        default { $script:DoInstallTester = $true; $script:DoInstallEndpoint = $true }
    }

    Invoke-DeploymentLocationPrompt

    Write-Host ""
    Show-Plan
    Write-Host ""
    $proceed = Invoke-AskYN "Proceed with this plan?" "y"
    if (-not $proceed) {
        Write-Host ""
        Write-Host "Installation cancelled."
        exit 0
    }
}

# ── Step helpers ───────────────────────────────────────────────────────────────
function Invoke-NextStep ($title) {
    $script:StepNum++
    Write-StepHeader $script:StepNum $title
}

# ══════════════════════════════════════════════════════════════════════════════
#  CLOUD CLI HELPERS
# ══════════════════════════════════════════════════════════════════════════════

# ── LAN deployment ────────────────────────────────────────────────────────────

function Test-LanSsh ($ip, $user, $port) {
    Write-Info "Testing SSH connection to ${user}@${ip}:${port}..."
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $null = & ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10 -o BatchMode=yes `
        -p $port "${user}@${ip}" "echo ok" 2>$null
    $ok = ($LASTEXITCODE -eq 0)
    $ErrorActionPreference = $prevErr

    if ($ok) {
        Write-Ok "SSH connection successful"
        return $true
    }

    Write-Host ""
    Write-Err "SSH connection to ${user}@${ip}:${port} failed."
    Write-Host ""
    Write-Host "  Troubleshooting steps:" -ForegroundColor White
    Write-Host ""
    Write-Host "  1. Verify the machine is reachable:"
    Write-Host "     ping $ip" -ForegroundColor DarkGray
    Write-Host ""
    Write-Host "  2. Ensure SSH server is running on the remote machine:"
    Write-Host "     # Linux:   sudo systemctl status sshd" -ForegroundColor DarkGray
    Write-Host "     # Windows: Get-Service sshd" -ForegroundColor DarkGray
    Write-Host "     # macOS:   System Settings > General > Sharing > Remote Login" -ForegroundColor DarkGray
    Write-Host ""
    Write-Host "  3. Copy your SSH key to the remote machine:"
    Write-Host "     ssh-copy-id -p $port ${user}@${ip}" -ForegroundColor DarkGray
    Write-Host ""
    Write-Host "  4. If using a non-standard port, ensure the firewall allows it:"
    Write-Host "     # Linux:   sudo ufw allow ${port}/tcp" -ForegroundColor DarkGray
    Write-Host "     # Windows: New-NetFirewallRule -Name sshd -DisplayName 'OpenSSH' -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort $port" -ForegroundColor DarkGray
    Write-Host ""
    Write-Host "  5. If the remote is Windows, enable OpenSSH Server:"
    Write-Host "     Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0" -ForegroundColor DarkGray
    Write-Host "     Start-Service sshd; Set-Service -Name sshd -StartupType Automatic" -ForegroundColor DarkGray
    Write-Host ""
    Write-Host "  6. Test manually:"
    Write-Host "     ssh -v -p $port ${user}@${ip}" -ForegroundColor DarkGray
    Write-Host ""
    return $false
}

function Get-LanRemoteOs ($ip, $user, $port) {
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $remoteOs = (& ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10 `
        -p $port "${user}@${ip}" "uname -s" 2>$null) -join ""
    $ErrorActionPreference = $prevErr

    $detected = "linux"
    switch -Wildcard ($remoteOs) {
        "Linux*"   { $detected = "linux" }
        "Darwin*"  { $detected = "linux" }
        "CYGWIN*"  { $detected = "windows" }
        "MINGW*"   { $detected = "windows" }
        "MSYS*"    { $detected = "windows" }
        "*_NT*"    { $detected = "windows" }
        default {
            # Fallback: try PowerShell
            $prevErr2 = $ErrorActionPreference
            $ErrorActionPreference = "Continue"
            $psTest = (& ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10 `
                -p $port "${user}@${ip}" "powershell -Command 'Write-Output windows'" 2>$null) -join ""
            $ErrorActionPreference = $prevErr2
            if ($psTest -match "windows") { $detected = "windows" }
        }
    }
    Write-Ok "Detected remote OS: $detected"
    return $detected
}

function Invoke-LanOptions ($role) {
    Write-Host ""
    Write-Section "LAN deployment -- networker-${role}"

    $ipVar   = "Lan${role}Ip"
    $userVar = "Lan${role}User"
    $portVar = "Lan${role}Port"
    $osVar   = "Lan${role}Os"

    # Capitalize role for variable names
    $roleUpper = (Get-Culture).TextInfo.ToTitleCase($role)
    $ipVar   = "Lan${roleUpper}Ip"
    $userVar = "Lan${roleUpper}User"
    $portVar = "Lan${roleUpper}Port"
    $osVar   = "Lan${roleUpper}Os"

    # IP address
    if (-not (Get-Variable -Name $ipVar -Scope Script -ValueOnly)) {
        $ipAns = Read-Host "  IP address or hostname"
        if ([string]::IsNullOrWhiteSpace($ipAns)) {
            Write-Err "IP address is required for LAN deployment."
            exit 1
        }
        Set-Variable -Name $ipVar -Scope Script -Value $ipAns
    }

    # SSH user
    if (-not (Get-Variable -Name $userVar -Scope Script -ValueOnly)) {
        $defaultUser = $env:USERNAME
        $userAns = Read-HostDefault "  SSH user [$defaultUser]" $defaultUser
        Set-Variable -Name $userVar -Scope Script -Value $userAns
    }

    # SSH port
    if ((Get-Variable -Name $portVar -Scope Script -ValueOnly) -eq "22") {
        $portAns = Read-HostDefault "  SSH port [22]" "22"
        Set-Variable -Name $portVar -Scope Script -Value $portAns
    }

    $ip   = Get-Variable -Name $ipVar   -Scope Script -ValueOnly
    $user = Get-Variable -Name $userVar -Scope Script -ValueOnly
    $port = Get-Variable -Name $portVar -Scope Script -ValueOnly

    # Test connection
    if (-not (Test-LanSsh $ip $user $port)) {
        exit 1
    }

    # Detect OS
    $os = Get-LanRemoteOs $ip $user $port
    Set-Variable -Name $osVar -Scope Script -Value $os
}

function Invoke-LanInstallBinaryLinux ($binary, $role) {
    $roleUpper = (Get-Culture).TextInfo.ToTitleCase($role)
    $ip   = Get-Variable -Name "Lan${roleUpper}Ip"   -Scope Script -ValueOnly
    $user = Get-Variable -Name "Lan${roleUpper}User" -Scope Script -ValueOnly
    $port = Get-Variable -Name "Lan${roleUpper}Port" -Scope Script -ValueOnly

    if ($port -eq "22") {
        Invoke-RemoteInstallBinary $binary $ip $user
    } else {
        # Use bootstrap approach for non-standard ports
        $component = if ($binary -eq "networker-tester") { "tester" } else { "endpoint" }
        $installerUrl = "https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.sh"

        Write-Info "Installing $binary on ${user}@${ip} via SSH (port $port)..."
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & ssh -o StrictHostKeyChecking=no -p $port "${user}@${ip}" `
            "curl -fsSL '${installerUrl}' -o /tmp/networker-install.sh && bash /tmp/networker-install.sh ${component} -y"
        $ErrorActionPreference = $prevErr
    }
}

function Invoke-LanInstallBinaryWindows ($binary, $role) {
    $roleUpper = (Get-Culture).TextInfo.ToTitleCase($role)
    $ip   = Get-Variable -Name "Lan${roleUpper}Ip"   -Scope Script -ValueOnly
    $user = Get-Variable -Name "Lan${roleUpper}User" -Scope Script -ValueOnly
    $port = Get-Variable -Name "Lan${roleUpper}Port" -Scope Script -ValueOnly

    $component = if ($binary -eq "networker-tester") { "tester" } else { "endpoint" }
    $installerUrl = "https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.ps1"

    Write-Info "Installing $binary on Windows host ${user}@${ip}..."
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & ssh -o StrictHostKeyChecking=no -p $port "${user}@${ip}" `
        "powershell -ExecutionPolicy Bypass -Command `"& { Invoke-WebRequest -Uri '${installerUrl}' -OutFile C:\networker-install.ps1; & C:\networker-install.ps1 -Component ${component} -AutoYes }`""
    $ErrorActionPreference = $prevErr

    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $ver = (& ssh -o StrictHostKeyChecking=no -p $port "${user}@${ip}" `
        "${binary} --version 2>`$null" 2>$null) -join ""
    $ErrorActionPreference = $prevErr
    if ($ver) { Write-Ok "$binary installed on remote host ($ver)" }
    else { Write-Warn "$binary install may have failed -- check host manually" }
}

function Invoke-LanCreateEndpointService ($role) {
    $roleUpper = (Get-Culture).TextInfo.ToTitleCase($role)
    $ip   = Get-Variable -Name "Lan${roleUpper}Ip"   -Scope Script -ValueOnly
    $user = Get-Variable -Name "Lan${roleUpper}User" -Scope Script -ValueOnly
    $port = Get-Variable -Name "Lan${roleUpper}Port" -Scope Script -ValueOnly

    if ($port -eq "22") {
        Invoke-RemoteCreateEndpointService $ip $user
    } else {
        $svcScript = @"
sudo useradd --system --no-create-home --shell /usr/sbin/nologin networker 2>/dev/null || true
sudo tee /etc/systemd/system/networker-endpoint.service > /dev/null <<'UNIT'
[Unit]
Description=Networker Endpoint
After=network.target
[Service]
User=networker
ExecStart=/usr/local/bin/networker-endpoint
Restart=always
RestartSec=5
Environment=RUST_LOG=info
[Install]
WantedBy=multi-user.target
UNIT
sudo systemctl daemon-reload
sudo systemctl enable networker-endpoint
sudo systemctl start networker-endpoint
if command -v iptables &>/dev/null; then
    sudo iptables -t nat -C PREROUTING -p tcp --dport 80 -j REDIRECT --to-port 8080 2>/dev/null || sudo iptables -t nat -A PREROUTING -p tcp --dport 80 -j REDIRECT --to-port 8080
    sudo iptables -t nat -C PREROUTING -p tcp --dport 443 -j REDIRECT --to-port 8443 2>/dev/null || sudo iptables -t nat -A PREROUTING -p tcp --dport 443 -j REDIRECT --to-port 8443
fi
"@
        $svcScript = $svcScript -replace "`r`n", "`n"
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $svcScript | & ssh -o StrictHostKeyChecking=no -p $port "${user}@${ip}" "bash -s"
        $ErrorActionPreference = $prevErr
        Start-Sleep -Seconds 2
        Write-Ok "networker-endpoint service enabled and started"
    }
}

function Invoke-LanCreateEndpointServiceWindows ($role) {
    $roleUpper = (Get-Culture).TextInfo.ToTitleCase($role)
    $ip   = Get-Variable -Name "Lan${roleUpper}Ip"   -Scope Script -ValueOnly
    $user = Get-Variable -Name "Lan${roleUpper}User" -Scope Script -ValueOnly
    $port = Get-Variable -Name "Lan${roleUpper}Port" -Scope Script -ValueOnly

    Write-Info "Creating networker-endpoint Windows service on ${user}@${ip}..."
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & ssh -o StrictHostKeyChecking=no -p $port "${user}@${ip}" `
        "powershell -ExecutionPolicy Bypass -Command `"& { if (-not (Get-Service networker-endpoint -EA SilentlyContinue)) { sc.exe create networker-endpoint binPath= 'C:\networker\networker-endpoint.exe' start= auto }; sc.exe start networker-endpoint 2>`$null; New-NetFirewallRule -Name 'NetworkerEndpoint-TCP' -DisplayName 'Networker Endpoint TCP' -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 8080,8443 -EA SilentlyContinue; New-NetFirewallRule -Name 'NetworkerEndpoint-UDP' -DisplayName 'Networker Endpoint UDP' -Enabled True -Direction Inbound -Protocol UDP -Action Allow -LocalPort 8443,9998,9999 -EA SilentlyContinue }`""
    $ErrorActionPreference = $prevErr
    Write-Ok "networker-endpoint Windows service created and started"
}

function Invoke-LanDeployTester {
    Invoke-NextStep "Deploy networker-tester to LAN host"
    if ($script:LanTesterOs -eq "windows") {
        Invoke-LanInstallBinaryWindows "networker-tester" "tester"
    } else {
        Invoke-LanInstallBinaryLinux "networker-tester" "tester"
    }
}

function Invoke-LanDeployEndpoint {
    Invoke-NextStep "Deploy networker-endpoint to LAN host"
    if ($script:LanEndpointOs -eq "windows") {
        Invoke-LanInstallBinaryWindows "networker-endpoint" "endpoint"
        Invoke-LanCreateEndpointServiceWindows "endpoint"
    } else {
        Invoke-LanInstallBinaryLinux "networker-endpoint" "endpoint"
        Invoke-LanCreateEndpointService "endpoint"
    }
    Invoke-GenerateConfig $script:LanEndpointIp
}

# ── Ensure Azure CLI ──────────────────────────────────────────────────────────
function Invoke-EnsureAzureCli {
    if (-not $script:AzureCliAvailable) {
        Write-Host ""
        Write-Warn "Azure CLI (az) is not installed."
        Write-Host ""
        Write-Host "  Install from: https://docs.microsoft.com/cli/azure/install-azure-cli"
        if ($script:WingetAvailable) {
            Write-Host "  Or:  winget install Microsoft.AzureCLI"
            Write-Host ""
            if (Invoke-AskYN "Install Azure CLI via winget now?" "y") {
                $prevErr = $ErrorActionPreference
                $ErrorActionPreference = "Continue"
                & winget install --id Microsoft.AzureCLI -e --source winget `
                    --accept-package-agreements --accept-source-agreements
                $ErrorActionPreference = $prevErr
                # Refresh PATH
                $machinePath = [System.Environment]::GetEnvironmentVariable("PATH", "Machine")
                $userPath    = [System.Environment]::GetEnvironmentVariable("PATH", "User")
                $env:PATH    = "$machinePath;$userPath"
                if (Get-Command az -ErrorAction SilentlyContinue) {
                    $script:AzureCliAvailable = $true
                    Write-Ok "Azure CLI installed"
                } else {
                    Write-Err "Azure CLI installation failed."
                    exit 1
                }
            } else {
                Write-Err "Azure CLI is required for Azure deployment."
                exit 1
            }
        } else {
            Write-Err "Azure CLI is required. Install from: https://aka.ms/installazurecliwindows"
            exit 1
        }
    }

    # Re-check login status (may already be logged in from another session)
    if (-not $script:AzureLoggedIn) {
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $null = & az account show --output none 2>&1
        $script:AzureLoggedIn = ($LASTEXITCODE -eq 0)
        $ErrorActionPreference = $prevErr
    }

    # Check service principal env vars
    if (-not $script:AzureLoggedIn -and $env:AZURE_CLIENT_ID -and $env:AZURE_CLIENT_SECRET -and $env:AZURE_TENANT_ID) {
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $null = & az login --service-principal `
            -u $env:AZURE_CLIENT_ID `
            -p $env:AZURE_CLIENT_SECRET `
            --tenant $env:AZURE_TENANT_ID `
            --output none 2>&1
        $script:AzureLoggedIn = ($LASTEXITCODE -eq 0)
        $ErrorActionPreference = $prevErr
        if ($script:AzureLoggedIn) {
            $sub = (& az account show --query name --output tsv 2>$null) -join ""
            if (-not $sub) { $sub = "unknown" }
            Write-Ok "Azure credentials found  (subscription: $sub)"
        }
    }

    if (-not $script:AzureLoggedIn) {
        Write-Host ""
        Write-Warn "Not logged in to Azure."
        Write-Host ""
        if (Invoke-AskYN "Log in to Azure now (opens browser)?" "y") {
            Write-Info "Logging in to Azure..."
            $prevErr = $ErrorActionPreference
            $ErrorActionPreference = "Continue"
            $loginOutput = & az login --use-device-code 2>&1
            $loginOutput | ForEach-Object { Write-Host $_ }
            $script:AzureLoggedIn = ($LASTEXITCODE -eq 0)

            # If login failed (e.g. MFA tenant, no subscriptions found), auto-detect tenant and retry
            if (-not $script:AzureLoggedIn) {
                # Try to extract tenant ID from az login output like:
                #   "please use `az login --tenant TENANT_ID`."
                #   "1ecbc8ed-6353-... 'Tenant Name'"
                $tenantId = ""
                $loginText = ($loginOutput | Out-String)
                # Match tenant GUID on a line by itself (az lists failed tenants one per line)
                if ($loginText -match '(?m)^([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\s') {
                    $tenantId = $Matches[1]
                }
                Write-Host ""
                Write-Warn "Default login failed -- this often happens when your subscription"
                Write-Warn "is in a tenant that requires MFA or when no subscriptions are found."
                Write-Host ""
                if ($tenantId) {
                    Write-Info "Detected tenant: $tenantId -- retrying with --tenant flag..."
                    & az login --use-device-code --tenant $tenantId
                    $script:AzureLoggedIn = ($LASTEXITCODE -eq 0)
                } else {
                    $tenantId = Read-Host "  Enter tenant ID to retry (or press Enter to abort)"
                    if ($tenantId) {
                        Write-Info "Logging in to tenant $tenantId..."
                        & az login --use-device-code --tenant $tenantId
                        $script:AzureLoggedIn = ($LASTEXITCODE -eq 0)
                    }
                }
            }
            $ErrorActionPreference = $prevErr
            if ($script:AzureLoggedIn) {
                Write-Ok "Logged in to Azure"
            } else {
                Write-Err "Azure login failed."
                exit 1
            }
        } else {
            Write-Err "Azure login required for deployment."
            exit 1
        }
    }
}

# ── Ensure AWS CLI ────────────────────────────────────────────────────────────
function Invoke-EnsureAwsCli {
    if (-not $script:AwsCliAvailable) {
        Write-Host ""
        Write-Warn "AWS CLI is not installed."
        Write-Host "  Install from: https://aws.amazon.com/cli/"
        if ($script:WingetAvailable) {
            Write-Host "  Or:  winget install Amazon.AWSCLI"
            Write-Host ""
            if (Invoke-AskYN "Install AWS CLI via winget now?" "y") {
                $prevErr = $ErrorActionPreference
                $ErrorActionPreference = "Continue"
                & winget install --id Amazon.AWSCLI -e --source winget `
                    --accept-package-agreements --accept-source-agreements
                $ErrorActionPreference = $prevErr
                $machinePath = [System.Environment]::GetEnvironmentVariable("PATH", "Machine")
                $userPath    = [System.Environment]::GetEnvironmentVariable("PATH", "User")
                $env:PATH    = "$machinePath;$userPath"
                if (Get-Command aws -ErrorAction SilentlyContinue) {
                    $script:AwsCliAvailable = $true
                    Write-Ok "AWS CLI installed"
                } else {
                    Write-Err "AWS CLI installation failed."
                    exit 1
                }
            } else {
                Write-Err "AWS CLI is required for AWS deployment."
                exit 1
            }
        } else {
            Write-Err "AWS CLI is required. Install from: https://aws.amazon.com/cli/"
            exit 1
        }
    }

    # Re-check: env vars may have been set after Discover-System ran
    if (-not $script:AwsLoggedIn -and $env:AWS_ACCESS_KEY_ID -and $env:AWS_SECRET_ACCESS_KEY) {
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $null = & aws sts get-caller-identity 2>&1
        $script:AwsLoggedIn = ($LASTEXITCODE -eq 0)
        $ErrorActionPreference = $prevErr
    }

    if ($script:AwsLoggedIn) {
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $awsArn = (& aws sts get-caller-identity --query Arn --output text 2>$null) -join ""
        $ErrorActionPreference = $prevErr
        if (-not $awsArn) { $awsArn = "unknown" }
        Write-Ok "AWS credentials found  ($awsArn)"
    } else {
        Write-Host ""
        Write-Warn "AWS CLI is not configured or credentials are not valid."
        Write-Host ""
        Write-Host "  Choose an authentication method:"
        Write-Host "    1) AWS SSO / Identity Center  (device code -- opens browser, no keys needed)"
        Write-Host "    2) Access keys                (AWS_ACCESS_KEY_ID + secret)"
        Write-Host ""
        if (Invoke-AskYN "Log in to AWS now?" "y") {
            Write-Host ""
            $authMethod = Read-HostDefault "  Auth method [1/2, default 1]" "1"

            if ($authMethod -eq "2") {
                Invoke-AwsLoginKeys
            } else {
                Invoke-AwsLoginSso
            }

            if (-not $script:AwsLoggedIn) {
                Write-Err "AWS authentication failed -- fix manually then re-run the installer."
                Write-Host "  SSO:         aws configure sso && aws sso login"
                Write-Host "  Access keys: aws configure"
                exit 1
            }
        } else {
            Write-Err "AWS credentials required for remote deployment."
            Write-Host "  SSO:         aws configure sso && aws sso login"
            Write-Host "  Access keys: aws configure"
            exit 1
        }
    }
}

# ── Internal: AWS SSO device-code login ──────────────────────────────────────
function Invoke-AwsLoginSso {
    Write-Host ""

    # Check if an SSO profile already exists
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $allProfiles = @(& aws configure list-profiles 2>$null)
    $ErrorActionPreference = $prevErr

    $ssoProfiles = @()
    foreach ($p in $allProfiles) {
        $p = $p.Trim()
        if (-not $p) { continue }
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $ssoUrl = (& aws configure get sso_start_url --profile $p 2>$null) -join ""
        $ErrorActionPreference = $prevErr
        if ($ssoUrl) { $ssoProfiles += $p }
    }

    if ($ssoProfiles.Count -eq 0) {
        Write-Info "Setting up AWS SSO profile (one-time setup)..."
        Write-Host ""
        Write-Host "  You will need your SSO start URL (e.g. https://my-org.awsapps.com/start)"
        Write-Host "  and your SSO region (e.g. us-east-1)."
        Write-Host ""
        & aws configure sso
    } else {
        if ($ssoProfiles.Count -eq 1) {
            $ssoProfile = $ssoProfiles[0]
            Write-Info "Using SSO profile: $ssoProfile"
        } else {
            Write-Host "  Existing SSO profiles:"
            for ($i = 0; $i -lt $ssoProfiles.Count; $i++) {
                Write-Host ("    {0}) {1}" -f ($i+1), $ssoProfiles[$i])
            }
            $newIdx = $ssoProfiles.Count + 1
            Write-Host "    $newIdx) Configure a new SSO profile"
            Write-Host ""
            $choice = Read-HostDefault "  Select profile [1]" "1"
            $idx = 0
            if ([int]::TryParse($choice, [ref]$idx) -and $idx -eq $newIdx) {
                & aws configure sso
                Invoke-AwsCheckIdentity
                return
            }
            if ([int]::TryParse($choice, [ref]$idx) -and $idx -ge 1 -and $idx -le $ssoProfiles.Count) {
                $ssoProfile = $ssoProfiles[$idx-1]
            } else {
                $ssoProfile = $ssoProfiles[0]
            }
        }

        Write-Info "Logging in via AWS SSO (device code)..."
        & aws sso login --profile $ssoProfile

        # Set the profile so subsequent aws commands use it
        $env:AWS_PROFILE = $ssoProfile
    }

    Invoke-AwsCheckIdentity
}

# ── Internal: AWS access-key login ───────────────────────────────────────────
function Invoke-AwsLoginKeys {
    Write-Host ""
    Write-Info "Running aws configure (access key + secret)..."
    Write-Host ""
    & aws configure

    Invoke-AwsCheckIdentity
}

# ── Internal: verify AWS identity after login ────────────────────────────────
function Invoke-AwsCheckIdentity {
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $null = & aws sts get-caller-identity 2>&1
    $script:AwsLoggedIn = ($LASTEXITCODE -eq 0)
    $ErrorActionPreference = $prevErr
    if ($script:AwsLoggedIn) {
        $awsAccount = (& aws sts get-caller-identity --query Account --output text 2>$null) -join ""
        if (-not $awsAccount) { $awsAccount = "unknown" }
        Write-Ok "AWS authenticated  (account: $awsAccount)"
    }
}

# ── Ensure GCP CLI ────────────────────────────────────────────────────────────
function Invoke-EnsureGcpCli {
    if (-not $script:GcpCliAvailable) {
        Write-Host ""
        Write-Warn "Google Cloud SDK (gcloud) is not installed."
        Write-Host "  Install from: https://cloud.google.com/sdk/docs/install"
        if ($script:WingetAvailable) {
            Write-Host "  Or:  winget install Google.CloudSDK"
            Write-Host ""
            if (Invoke-AskYN "Install Google Cloud SDK via winget now?" "y") {
                $prevErr = $ErrorActionPreference
                $ErrorActionPreference = "Continue"
                & winget install --id Google.CloudSDK -e --source winget `
                    --accept-package-agreements --accept-source-agreements
                $ErrorActionPreference = $prevErr
                # Refresh PATH to pick up newly installed gcloud
                $machinePath = [System.Environment]::GetEnvironmentVariable("PATH", "Machine")
                $userPath    = [System.Environment]::GetEnvironmentVariable("PATH", "User")
                $env:PATH    = "$machinePath;$userPath"
                if (Get-Command gcloud -ErrorAction SilentlyContinue) {
                    $script:GcpCliAvailable = $true
                    $ver = (& gcloud --version 2>$null | Select-Object -First 1) -join ""
                    Write-Ok "Google Cloud SDK installed  ($ver)"
                } else {
                    Write-Err "Google Cloud SDK installation failed -- install manually."
                    Write-Host "  https://cloud.google.com/sdk/docs/install"
                    exit 1
                }
            } else {
                Write-Err "Google Cloud SDK is required for GCP deployment."
                exit 1
            }
        } else {
            Write-Err "gcloud CLI is required. Install from: https://cloud.google.com/sdk/docs/install"
            exit 1
        }
    }

    if (-not $script:GcpLoggedIn) {
        # Re-check in case session is active
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $acct = (& gcloud config get-value account 2>$null) -join ""
        $ErrorActionPreference = $prevErr
        if ($acct -and $acct -ne "(unset)") {
            $script:GcpLoggedIn = $true
            Write-Ok "Logged in: $acct"
        }
    }

    # Check GOOGLE_APPLICATION_CREDENTIALS (service account key file)
    if (-not $script:GcpLoggedIn -and $env:GOOGLE_APPLICATION_CREDENTIALS -and (Test-Path $env:GOOGLE_APPLICATION_CREDENTIALS)) {
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & gcloud auth activate-service-account --key-file $env:GOOGLE_APPLICATION_CREDENTIALS --quiet 2>$null
        $acct = (& gcloud config get-value account 2>$null) -join ""
        $ErrorActionPreference = $prevErr
        if ($acct -and $acct -ne "(unset)") {
            $script:GcpLoggedIn = $true
            Write-Ok "GCP credentials found  ($acct)"
        }
    }

    if (-not $script:GcpLoggedIn) {
        Write-Host ""
        Write-Warn "Not logged in to GCP."
        Write-Host ""
        if (Invoke-AskYN "Log in to GCP now?" "y") {
            Write-Info "Logging in to GCP..."
            $prevErr = $ErrorActionPreference
            $ErrorActionPreference = "Continue"
            & gcloud auth login --no-launch-browser
            $acct = (& gcloud config get-value account 2>$null) -join ""
            $ErrorActionPreference = $prevErr
            if ($acct -and $acct -ne "(unset)") {
                $script:GcpLoggedIn = $true
                Write-Ok "Logged in: $acct"
            } else {
                Write-Err "GCP login failed."
                exit 1
            }
        } else {
            Write-Err "GCP login required for deployment."
            exit 1
        }
    }
}

# ══════════════════════════════════════════════════════════════════════════════
#  CLOUD OPTION PROMPTS
# ══════════════════════════════════════════════════════════════════════════════

function Invoke-AzureOptions ($component) {
    # Guard: skip if already configured (e.g. called from Invoke-DeploymentLocationPrompt)
    if ($component -eq "tester" -and $script:AzureTesterVm)   { return }
    if ($component -ne "tester" -and $script:AzureEndpointVm) { return }

    $title = if ($component -eq "tester") { "networker-tester" } else { "networker-endpoint" }
    Write-Section "Azure options for $title"
    Write-Host ""

    # Region (shared; ask once)
    if (-not $script:AzureRegionAsked) {
        $script:AzureRegionAsked = $true
        $regions = @(
            @{ Code="eastus";        Label="East US (Virginia)" },
            @{ Code="westus2";       Label="West US 2 (Washington)" },
            @{ Code="westeurope";    Label="West Europe (Netherlands)" },
            @{ Code="northeurope";   Label="North Europe (Ireland)" },
            @{ Code="southeastasia"; Label="Southeast Asia (Singapore)" },
            @{ Code="australiaeast"; Label="Australia East (NSW)" },
            @{ Code="uksouth";       Label="UK South (London)" },
            @{ Code="japaneast";     Label="Japan East (Tokyo)" }
        )
        Write-Host "  Azure region:"
        for ($i = 0; $i -lt $regions.Count; $i++) {
            $tag = if ($regions[$i].Code -eq $script:AzureRegion) { "  [current]" } else { "" }
            Write-Host ("    {0}) {1,-20} {2}{3}" -f ($i+1), $regions[$i].Code, $regions[$i].Label, $tag)
        }
        Write-Host ""
        $regAns = Read-HostDefault "  Choice [1]" "1"
        $idx = 0; if ([int]::TryParse($regAns, [ref]$idx) -and $idx -ge 1 -and $idx -le $regions.Count) {
            $script:AzureRegion = $regions[$idx-1].Code
        }
        Write-Ok "Region: $($script:AzureRegion)"
        Write-Host ""
    } else {
        Write-Info "Region: $($script:AzureRegion)  (shared with other Azure VM)"
        Write-Host ""
    }

    # VM size
    Write-Host "  VM size:"
    Write-Host "    1) Standard_B1s     1 vCPU,  1 GB RAM  ~`$7/mo"
    Write-Host "    2) Standard_B2s     2 vCPU,  4 GB RAM  ~`$30/mo  [default]"
    Write-Host "    3) Standard_D2s_v3  2 vCPU,  8 GB RAM  ~`$70/mo"
    Write-Host "    4) Standard_D4s_v3  4 vCPU, 16 GB RAM  ~`$140/mo"
    Write-Host ""
    $sizeAns = Read-HostDefault "  Choice [2]" "2"
    $chosenSize = switch ($sizeAns) {
        "1" { "Standard_B1s" }
        "3" { "Standard_D2s_v3" }
        "4" { "Standard_D4s_v3" }
        default { "Standard_B2s" }
    }
    Write-Host ""

    # OS choice
    Write-Host "  Operating System:"
    Write-Host "    1) Ubuntu 22.04 LTS  (Linux)    [default]"
    Write-Host "    2) Windows Server 2022"
    Write-Host ""
    $osAns = Read-HostDefault "  Choice [1]" "1"
    $chosenOs = if ($osAns -eq "2") { "windows" } else { "linux" }
    Write-Host ""

    # Auto-shutdown
    if (-not $script:AzureShutdownAsked) {
        $script:AzureShutdownAsked = $true
        Write-Host "  Auto-shutdown policy (avoids unexpected charges):"
        Write-Host "    1) Shut down at 11 PM EST (04:00 UTC) daily  [default]"
        Write-Host "    2) Leave running -- I will stop/delete manually"
        Write-Host ""
        $sdAns = Read-HostDefault "  Choice [1]" "1"
        if ($sdAns -eq "2") {
            $script:AzureAutoShutdown = "no"
            Write-Warn "VMs will keep running -- remember to delete them when done!"
        } else {
            $script:AzureAutoShutdown = "yes"
            Write-Ok "Auto-shutdown: 04:00 UTC (11 PM EST) daily"
        }
        Write-Host ""
    }

    # Names
    $suggestedRg = "nwk-$component-$($script:AzureRegion)"
    $rg = Read-HostDefault "  Resource group name [$suggestedRg]" $suggestedRg
    $suggestedVm = "$suggestedRg-vm"
    $vm = Read-HostDefault "  VM name [$suggestedVm]" $suggestedVm

    if ($component -eq "tester") {
        $script:AzureTesterRg = $rg; $script:AzureTesterVm = $vm
        $script:AzureTesterSize = $chosenSize; $script:AzureTesterOs = $chosenOs
    } else {
        $script:AzureEndpointRg = $rg; $script:AzureEndpointVm = $vm
        $script:AzureEndpointSize = $chosenSize; $script:AzureEndpointOs = $chosenOs
    }
    Write-Ok "OS: $chosenOs  |  Size: $chosenSize  |  RG: $rg  |  VM: $vm"
    Write-Host ""
}

function Invoke-AwsOptions ($component) {
    # Guard: skip if already configured
    if ($component -eq "tester" -and $script:AwsTesterOptionsAsked)   { return }
    if ($component -ne "tester" -and $script:AwsEndpointOptionsAsked) { return }

    $title = if ($component -eq "tester") { "networker-tester" } else { "networker-endpoint" }
    Write-Section "AWS options for $title"
    Write-Host ""

    if (-not $script:AwsRegionAsked) {
        $script:AwsRegionAsked = $true
        $regions = @(
            @{ Code="us-east-1";      Label="US East (N. Virginia)" },
            @{ Code="us-west-2";      Label="US West (Oregon)" },
            @{ Code="eu-west-1";      Label="EU West (Ireland)" },
            @{ Code="eu-central-1";   Label="EU Central (Frankfurt)" },
            @{ Code="ap-southeast-1"; Label="Asia Pacific (Singapore)" },
            @{ Code="ap-northeast-1"; Label="Asia Pacific (Tokyo)" },
            @{ Code="ap-southeast-2"; Label="Asia Pacific (Sydney)" },
            @{ Code="sa-east-1";      Label="South America (Sao Paulo)" }
        )
        Write-Host "  AWS region:"
        for ($i = 0; $i -lt $regions.Count; $i++) {
            $tag = if ($regions[$i].Code -eq $script:AwsRegion) { "  [current]" } else { "" }
            Write-Host ("    {0}) {1,-20} {2}{3}" -f ($i+1), $regions[$i].Code, $regions[$i].Label, $tag)
        }
        Write-Host ""
        $regAns = Read-HostDefault "  Choice [1]" "1"
        $idx = 0; if ([int]::TryParse($regAns, [ref]$idx) -and $idx -ge 1 -and $idx -le $regions.Count) {
            $script:AwsRegion = $regions[$idx-1].Code
        }
        Write-Ok "Region: $($script:AwsRegion)"
        Write-Host ""
    } else {
        Write-Info "Region: $($script:AwsRegion)  (shared with other AWS instance)"
        Write-Host ""
    }

    Write-Host "  EC2 instance type:"
    Write-Host "    1) t3.micro   2 vCPU,  1 GB RAM  ~`$7/mo"
    Write-Host "    2) t3.small   2 vCPU,  2 GB RAM  ~`$15/mo  [default]"
    Write-Host "    3) t3.medium  2 vCPU,  4 GB RAM  ~`$30/mo"
    Write-Host "    4) t3.large   2 vCPU,  8 GB RAM  ~`$60/mo"
    Write-Host ""
    $typeAns = Read-HostDefault "  Choice [2]" "2"
    $chosenType = switch ($typeAns) {
        "1" { "t3.micro" }
        "3" { "t3.medium" }
        "4" { "t3.large" }
        default { "t3.small" }
    }
    Write-Host ""

    # OS
    Write-Host "  Operating System:"
    Write-Host "    1) Ubuntu 22.04  (Linux)  [default]"
    Write-Host "    2) Windows Server 2022"
    Write-Host ""
    $osAns = Read-HostDefault "  Choice [1]" "1"
    $chosenOs = if ($osAns -eq "2") { "windows" } else { "linux" }
    Write-Host ""

    # Auto-shutdown
    if (-not $script:AwsShutdownAsked) {
        $script:AwsShutdownAsked = $true
        Write-Host "  Auto-shutdown policy (avoids unexpected charges):"
        Write-Host "    1) Shut down at 11 PM EST (04:00 UTC) daily  [default]"
        Write-Host "    2) Leave running -- I will terminate manually"
        Write-Host ""
        $sdAns = Read-HostDefault "  Choice [1]" "1"
        if ($sdAns -eq "2") { $script:AwsAutoShutdown = "no"; Write-Warn "Instance will keep running!" }
        else { $script:AwsAutoShutdown = "yes"; Write-Ok "Auto-shutdown: 04:00 UTC (11 PM EST) daily" }
        Write-Host ""
    }

    $suggested = "networker-$component-$($script:AwsRegion)"
    $name = Read-HostDefault "  Instance name tag [$suggested]" $suggested

    if ($component -eq "tester") {
        $script:AwsTesterName = $name; $script:AwsTesterType = $chosenType; $script:AwsTesterOs = $chosenOs
        $script:AwsTesterOptionsAsked = $true
    } else {
        $script:AwsEndpointName = $name; $script:AwsEndpointType = $chosenType; $script:AwsEndpointOs = $chosenOs
        $script:AwsEndpointOptionsAsked = $true
    }
    Write-Ok "OS: $chosenOs  |  Type: $chosenType  |  Name: $name"
    Write-Host ""
}

# ── Resolve GCP project number to project ID ─────────────────────────────────
function Invoke-GcpResolveProject {
    if ($script:GcpProject -match '^\d+$') {
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $projId = (& gcloud projects describe $script:GcpProject `
            --format "value(projectId)" 2>$null) -join ""
        $ErrorActionPreference = $prevErr
        if ($projId) {
            Write-Dim "Resolved project number $($script:GcpProject) -> $projId"
            $script:GcpProject = $projId
            $prevErr2 = $ErrorActionPreference
            $ErrorActionPreference = "Continue"
            $null = & gcloud config set project $script:GcpProject 2>$null
            $ErrorActionPreference = $prevErr2
        }
    }
}

function Invoke-GcpOptions ($component) {
    # Guard: skip if already configured
    if ($component -eq "tester" -and $script:GcpTesterOptionsAsked)   { return }
    if ($component -ne "tester" -and $script:GcpEndpointOptionsAsked) { return }

    $title = if ($component -eq "tester") { "networker-tester" } else { "networker-endpoint" }
    Write-Section "GCP options for $title"
    Write-Host ""

    # Project
    if (-not $script:GcpProject) {
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $script:GcpProject = ((& gcloud config get-value project 2>$null) -join "").Trim()
        $ErrorActionPreference = $prevErr
        if ($script:GcpProject -eq "(unset)") { $script:GcpProject = "" }
    }
    if (-not $script:GcpProject) {
        $script:GcpProject = Read-HostDefault "  Enter your GCP project ID" ""
        if (-not $script:GcpProject) { Write-Err "GCP project ID is required."; exit 1 }
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & gcloud config set project $script:GcpProject 2>$null
        $ErrorActionPreference = $prevErr
    }
    Invoke-GcpResolveProject
    Write-Ok "Project: $($script:GcpProject)"
    Write-Host ""

    # Zone
    if (-not $script:GcpRegionAsked) {
        $script:GcpRegionAsked = $true
        $zones = @(
            @{ Code="us-central1-a";        Label="US Central (Iowa)" },
            @{ Code="us-east1-b";            Label="US East (South Carolina)" },
            @{ Code="us-west1-a";            Label="US West (Oregon)" },
            @{ Code="europe-west1-b";        Label="Europe West (Belgium)" },
            @{ Code="europe-west2-a";        Label="Europe West (London)" },
            @{ Code="asia-east1-a";          Label="Asia East (Taiwan)" },
            @{ Code="asia-northeast1-a";     Label="Asia NE (Tokyo)" },
            @{ Code="australia-southeast1-a";Label="Australia SE (Sydney)" }
        )
        Write-Host "  GCP zone:"
        for ($i = 0; $i -lt $zones.Count; $i++) {
            $tag = if ($zones[$i].Code -eq $script:GcpZone) { "  [current]" } else { "" }
            Write-Host ("    {0}) {1,-28} {2}{3}" -f ($i+1), $zones[$i].Code, $zones[$i].Label, $tag)
        }
        Write-Host ""
        $zoneAns = Read-HostDefault "  Choice [1]" "1"
        $idx = 0; if ([int]::TryParse($zoneAns, [ref]$idx) -and $idx -ge 1 -and $idx -le $zones.Count) {
            $script:GcpZone = $zones[$idx-1].Code
        }
        $script:GcpRegion = $script:GcpZone -replace '-[a-z]$',''
        Write-Ok "Zone: $($script:GcpZone)  (region: $($script:GcpRegion))"
        Write-Host ""
    } else {
        Write-Info "Zone: $($script:GcpZone)  (shared with other GCP instance)"
        Write-Host ""
    }

    # Machine type
    Write-Host "  GCE machine type:"
    Write-Host "    1) e2-micro      2 vCPU (shared), 1 GB RAM  ~`$7/mo"
    Write-Host "    2) e2-small      2 vCPU (shared), 2 GB RAM  ~`$15/mo  [default]"
    Write-Host "    3) e2-medium     2 vCPU (shared), 4 GB RAM  ~`$27/mo"
    Write-Host "    4) e2-standard-2 2 vCPU,          8 GB RAM  ~`$49/mo"
    Write-Host ""
    $typeAns = Read-HostDefault "  Choice [2]" "2"
    $chosenType = switch ($typeAns) {
        "1" { "e2-micro" }
        "3" { "e2-medium" }
        "4" { "e2-standard-2" }
        default { "e2-small" }
    }
    Write-Host ""

    # OS choice
    Write-Host "  Operating System:"
    Write-Host "    1) Ubuntu 22.04 LTS  (Linux)    [default]"
    Write-Host "    2) Windows Server 2022"
    Write-Host ""
    $osAns = Read-HostDefault "  Choice [1]" "1"
    $chosenOs = if ($osAns -eq "2") { "windows" } else { "linux" }
    Write-Host ""

    # Auto-shutdown
    if (-not $script:GcpShutdownAsked) {
        $script:GcpShutdownAsked = $true
        Write-Host "  Auto-shutdown policy (avoids unexpected charges):"
        Write-Host "    1) Shut down at 11 PM EST (04:00 UTC) daily  [default]"
        Write-Host "    2) Leave running -- I will stop/delete manually"
        Write-Host ""
        $sdAns = Read-HostDefault "  Choice [1]" "1"
        if ($sdAns -eq "2") { $script:GcpAutoShutdown = "no"; Write-Warn "Instance will keep running!" }
        else { $script:GcpAutoShutdown = "yes"; Write-Ok "Auto-shutdown: 04:00 UTC (11 PM EST) daily" }
        Write-Host ""
    }

    $regionTag = $script:GcpRegion
    $suggested = "networker-$component-$regionTag"
    $name = Read-HostDefault "  Instance name [$suggested]" $suggested

    if ($component -eq "tester") {
        $script:GcpTesterName = $name; $script:GcpTesterMachineType = $chosenType
        $script:GcpTesterOs = $chosenOs; $script:GcpTesterOptionsAsked = $true
    } else {
        $script:GcpEndpointName = $name; $script:GcpEndpointMachineType = $chosenType
        $script:GcpEndpointOs = $chosenOs; $script:GcpEndpointOptionsAsked = $true
    }
    Write-Ok "OS: $chosenOs  |  Type: $chosenType  |  Name: $name  |  Zone: $($script:GcpZone)"
    Write-Host ""
}

# ══════════════════════════════════════════════════════════════════════════════
#  LOCAL INSTALL STEPS
# ══════════════════════════════════════════════════════════════════════════════

function Invoke-DownloadReleaseStep ($binary) {
    Invoke-NextStep "Download $binary"
    $archive = "$binary-$($script:ReleaseTarget).zip"
    Write-Info "Fetching $archive from latest GitHub release..."

    $tmpDir = Join-Path $env:TEMP ("nw-install-" + [System.IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Force $tmpDir | Out-Null

    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & gh release download --repo $RepoGh --latest `
        --pattern $archive --dir $tmpDir --clobber
    $ok = ($LASTEXITCODE -eq 0)
    $ErrorActionPreference = $prevErr

    if (-not $ok) {
        Write-Host ""
        Write-Err "gh release download failed."
        Write-Host "  Expected asset: $archive"
        Remove-Item $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
        exit 1
    }

    New-Item -ItemType Directory -Force $CargoBin | Out-Null
    Expand-Archive -Path "$tmpDir\$archive" -DestinationPath $CargoBin -Force
    Remove-Item $tmpDir -Recurse -Force -ErrorAction SilentlyContinue

    $installedCmd  = Get-Command $binary -ErrorAction SilentlyContinue
    $installedPath = if ($installedCmd) { $installedCmd.Source } else { "$CargoBin\$binary.exe" }
    $installedVer  = if ($installedCmd) { (& $binary --version 2>&1) } else { "unknown" }
    Write-Host ""
    Write-Ok "$binary installed -> $installedPath  ($installedVer)"
}

function Invoke-MsvcInstallStep {
    Invoke-NextStep "Install Visual C++ Build Tools"
    Write-Info "Installing MSVC build tools via winget..."
    Write-Dim "Includes the C++ linker (link.exe) required to compile Rust on Windows."
    Write-Host ""

    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & winget install --id Microsoft.VisualStudio.2022.BuildTools -e --source winget `
        --accept-package-agreements --accept-source-agreements `
        --override "--wait --quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $prevErr

    if ($exitCode -ne 0) {
        Write-Err "winget install failed (exit code $exitCode)."
        Write-Host "  Install manually from: https://aka.ms/vs/buildtools"
        exit 1
    }

    $vswhereExe = Join-Path ([System.Environment]::GetFolderPath('ProgramFilesX86')) `
                             "Microsoft Visual Studio\Installer\vswhere.exe"
    $vsPath  = $null
    $elapsed = 0
    $timeout = 900

    Write-Info "Waiting for Visual Studio installation to complete..."
    while ($elapsed -lt $timeout) {
        if (Test-Path $vswhereExe) {
            $raw = & $vswhereExe -latest -products * `
                -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
                -property installationPath 2>&1
            if (-not [string]::IsNullOrWhiteSpace([string]$raw)) {
                $vsPath = ([string]$raw).Trim()
                break
            }
        }
        Start-Sleep -Seconds 15
        $elapsed += 15
        Write-Host ("    still installing... ({0}s)" -f $elapsed) -ForegroundColor DarkGray
    }

    if (-not $vsPath) {
        Write-Warn "VS Build Tools did not finish within ${timeout}s."
        exit 1
    }

    try {
        $vcvars = Join-Path $vsPath "VC\Auxiliary\Build\vcvars64.bat"
        if (Test-Path $vcvars) {
            Write-Info "Loading MSVC environment (vcvars64.bat)..."
            $envOutput = cmd.exe /c "`"$vcvars`" > NUL 2>&1 && set" 2>&1
            foreach ($line in $envOutput) {
                if ([string]$line -match "^([^=]+)=(.+)$") {
                    [System.Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], "Process")
                }
            }
            $script:MsvcAvailable = $true
            Write-Ok "VC++ Build Tools installed and loaded"
        } else {
            Write-Warn "vcvars64.bat not found. Reopen terminal and re-run."
            exit 1
        }
    } catch {
        Write-Warn ("Could not load MSVC environment: {0}" -f $_.Exception.Message)
        exit 1
    }
}

function Invoke-ChromeInstallStep {
    Invoke-NextStep "Install Chrome (browser probe)"
    Write-Info "Installing Google Chrome via winget..."
    Write-Host ""

    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & winget install --id Google.Chrome -e --source winget `
        --accept-package-agreements --accept-source-agreements
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $prevErr

    if ($exitCode -ne 0) {
        Write-Warn "Chrome install failed -- browser probe will not be compiled."
        $script:ChromeAvailable = $false
        return
    }

    $script:ChromePath = Get-ChromePath
    if ($script:ChromePath) {
        $script:ChromeAvailable = $true
        Write-Ok "Chrome installed: $($script:ChromePath)"
    } else {
        $script:ChromeAvailable = $true
        Write-Warn "Chrome installed but not yet detectable."
    }
}

function Invoke-GitInstallStep {
    Invoke-NextStep "Install git"
    Write-Info "Installing git via winget..."
    Write-Host ""

    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & winget install --id Git.Git -e --source winget `
        --accept-package-agreements --accept-source-agreements
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $prevErr

    if ($exitCode -ne 0) {
        Write-Err "winget install failed (exit code $exitCode)."
        Write-Host "  Install Git from: https://git-scm.com/"
        exit 1
    }

    $machinePath = [System.Environment]::GetEnvironmentVariable("PATH", "Machine")
    $userPath    = [System.Environment]::GetEnvironmentVariable("PATH", "User")
    $env:PATH    = "$machinePath;$userPath"

    $gitCmd = Get-Command git -ErrorAction SilentlyContinue
    if ($gitCmd) {
        $script:GitAvailable = $true
        Write-Ok ("git installed: " + (& git --version 2>&1))
    } else {
        Write-Warn "git installed but not yet in PATH."
    }
}

function Invoke-RustInstallStep {
    Invoke-NextStep "Install Rust via rustup"

    $arch      = $env:PROCESSOR_ARCHITECTURE
    $rustupUrl = if ($arch -eq "ARM64") { "https://win.rustup.rs/aarch64" } else { "https://win.rustup.rs/x86_64" }
    $rustupExe = Join-Path $env:TEMP "rustup-init.exe"

    Write-Info "Downloading rustup from $rustupUrl ..."
    Invoke-WebRequest -Uri $rustupUrl -OutFile $rustupExe -UseBasicParsing
    & $rustupExe -y --no-modify-path
    Remove-Item $rustupExe -Force -ErrorAction SilentlyContinue

    if ($env:PATH -notlike "*$CargoBin*") { $env:PATH = "$CargoBin;$env:PATH" }
    $script:RustVer = (& rustc --version 2>&1)
    Write-Ok ("Rust installed: " + $script:RustVer)
}

function Invoke-EnsureCargoEnv {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        if ($env:PATH -notlike "*$CargoBin*") { $env:PATH = "$CargoBin;$env:PATH" }
    }
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Write-Err "cargo not found -- cannot install binaries."
        exit 1
    }
}

function Invoke-CargoInstallStep ($binary) {
    Invoke-NextStep "Install $binary"
    Write-Info "Building and installing $binary from source..."
    Write-Dim "Compiling from GitHub -- may take a few minutes on first build."

    if (-not $script:MsvcAvailable) {
        Write-Host ""
        Write-Warn "VC++ Build Tools not detected -- cargo will likely fail."
    } else {
        Write-Host ""
    }

    if ($script:ChromeAvailable -and $binary -eq "networker-tester") {
        Write-Info "Chrome detected -- compiling with browser probe support."
    }

    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    if ($script:ChromeAvailable -and $binary -eq "networker-tester") {
        & cargo install --git $RepoHttps $binary --force --features browser
    } else {
        & cargo install --git $RepoHttps $binary --force
    }
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $prevErr

    if ($exitCode -ne 0) {
        Write-Err "cargo install failed (exit code $exitCode)."
        exit 1
    }

    $installedCmd  = Get-Command $binary -ErrorAction SilentlyContinue
    $installedPath = if ($installedCmd) { $installedCmd.Source } else { "$CargoBin\$binary.exe" }
    $installedVer  = if ($installedCmd) { (& $binary --version 2>&1) } else { "unknown" }
    Write-Host ""
    Write-Ok "$binary installed -> $installedPath  ($installedVer)"
}

# ══════════════════════════════════════════════════════════════════════════════
#  CLOUD DEPLOYMENT STEPS
# ══════════════════════════════════════════════════════════════════════════════

# ── VM existence check (reuse/rename/delete) ──────────────────────────────────
# Returns $true if the VM was reused (caller should skip creation).
function Invoke-VmExistsCheck {
    param([string]$Provider, [string]$Label, [string]$Name)

    $exists = $false
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"

    switch ($Provider) {
        "azure" {
            $rg = if ($Label -eq "tester") { $script:AzureTesterRg } else { $script:AzureEndpointRg }
            try {
                $null = & az vm show --resource-group $rg --name $Name --output none 2>&1
                $exists = ($LASTEXITCODE -eq 0)
            } catch { $exists = $false }
        }
        "aws" {
            try {
                $existingId = (& aws ec2 describe-instances `
                    --region $script:AwsRegion `
                    --filters "Name=tag:Name,Values=$Name" "Name=instance-state-name,Values=running,stopped,pending" `
                    --query "Reservations[0].Instances[0].InstanceId" `
                    --output text 2>$null) -join ""
                $exists = ($existingId -and $existingId -ne "None")
            } catch { $exists = $false }
        }
        "gcp" {
            try {
                $null = & gcloud compute instances describe $Name `
                    --project $script:GcpProject --zone $script:GcpZone 2>&1
                $exists = ($LASTEXITCODE -eq 0)
            } catch { $exists = $false }
        }
    }
    $ErrorActionPreference = $prevErr

    if (-not $exists) { return $false }

    Write-Host ""
    Write-Warn "$Provider instance '$Name' already exists."
    Write-Host ""
    Write-Host "    1) Reuse existing instance  [default]"
    Write-Host "    2) Pick a different name"
    Write-Host "    3) Delete and recreate"
    Write-Host ""
    $choice = Read-HostDefault "  Choice [1]" "1"

    switch ($choice) {
        "1" {
            # Get IP and return reused
            $ip = ""
            $prevErr2 = $ErrorActionPreference
            $ErrorActionPreference = "Continue"
            switch ($Provider) {
                "azure" {
                    $rg = if ($Label -eq "tester") { $script:AzureTesterRg } else { $script:AzureEndpointRg }
                    # Check power state and start if deallocated/stopped
                    $powerState = (& az vm show --resource-group $rg --name $Name `
                        --show-details --query powerState -o tsv 2>$null) -join ""
                    if ($powerState -and $powerState -ne "VM running") {
                        Write-Info "VM is '$powerState' -- starting it..."
                        $null = & az vm start --resource-group $rg --name $Name --output none 2>&1
                        Write-Ok "VM started"
                    }
                    $ip = (& az vm show --resource-group $rg --name $Name `
                        --show-details --query publicIps -o tsv 2>$null) -join ""
                }
                "aws" {
                    $ip = (& aws ec2 describe-instances `
                        --region $script:AwsRegion `
                        --filters "Name=tag:Name,Values=$Name" "Name=instance-state-name,Values=running,stopped,pending" `
                        --query "Reservations[0].Instances[0].PublicIpAddress" `
                        --output text 2>$null) -join ""
                    if (-not $ip -or $ip -eq "None") {
                        # Start stopped instance
                        Write-Info "Starting stopped instance..."
                        $iid = (& aws ec2 describe-instances `
                            --region $script:AwsRegion `
                            --filters "Name=tag:Name,Values=$Name" "Name=instance-state-name,Values=stopped" `
                            --query "Reservations[0].Instances[0].InstanceId" `
                            --output text 2>$null) -join ""
                        if ($iid -and $iid -ne "None") {
                            $null = & aws ec2 start-instances --region $script:AwsRegion --instance-ids $iid --output text 2>&1
                            $null = & aws ec2 wait instance-running --region $script:AwsRegion --instance-ids $iid 2>&1
                            $ip = (& aws ec2 describe-instances `
                                --region $script:AwsRegion --instance-ids $iid `
                                --query "Reservations[0].Instances[0].PublicIpAddress" `
                                --output text 2>$null) -join ""
                            if ($Label -eq "tester") { $script:AwsTesterInstanceId = $iid }
                            else { $script:AwsEndpointInstanceId = $iid }
                        }
                    }
                }
                "gcp" {
                    $ip = (& gcloud compute instances describe $Name `
                        --project $script:GcpProject --zone $script:GcpZone `
                        --format "get(networkInterfaces[0].accessConfigs[0].natIP)" 2>$null) -join ""
                }
            }
            $ErrorActionPreference = $prevErr2

            if (-not $ip -or $ip -eq "None") {
                Write-Err "Failed to retrieve instance public IP."
                exit 1
            }
            # Store IP
            if ($Label -eq "tester") {
                switch ($Provider) {
                    "azure" { $script:AzureTesterIp = $ip }
                    "aws"   { $script:AwsTesterIp = $ip }
                    "gcp"   { $script:GcpTesterIp = $ip }
                }
            } else {
                switch ($Provider) {
                    "azure" { $script:AzureEndpointIp = $ip }
                    "aws"   { $script:AwsEndpointIp = $ip }
                    "gcp"   { $script:GcpEndpointIp = $ip }
                }
            }
            Write-Ok "Reusing instance '$Name' -- Public IP: $ip"
            return $true
        }
        "2" {
            $newName = Read-Host "  New instance name"
            if (-not $newName) { Write-Err "Instance name is required."; exit 1 }
            # Update the name in state
            if ($Label -eq "tester") {
                switch ($Provider) {
                    "azure" { $script:AzureTesterVm = $newName }
                    "aws"   { $script:AwsTesterName = $newName }
                    "gcp"   { $script:GcpTesterName = $newName }
                }
            } else {
                switch ($Provider) {
                    "azure" { $script:AzureEndpointVm = $newName }
                    "aws"   { $script:AwsEndpointName = $newName }
                    "gcp"   { $script:GcpEndpointName = $newName }
                }
            }
            return $false
        }
        "3" {
            Write-Info "Deleting instance '$Name'..."
            $prevErr2 = $ErrorActionPreference
            $ErrorActionPreference = "Continue"
            switch ($Provider) {
                "azure" {
                    $rg = if ($Label -eq "tester") { $script:AzureTesterRg } else { $script:AzureEndpointRg }
                    $null = & az vm delete --resource-group $rg --name $Name --yes --output none 2>&1
                }
                "aws" {
                    $iid = (& aws ec2 describe-instances `
                        --region $script:AwsRegion `
                        --filters "Name=tag:Name,Values=$Name" "Name=instance-state-name,Values=running,stopped,pending" `
                        --query "Reservations[0].Instances[0].InstanceId" `
                        --output text 2>$null) -join ""
                    if ($iid -and $iid -ne "None") {
                        $null = & aws ec2 terminate-instances --region $script:AwsRegion --instance-ids $iid --output text 2>&1
                        $null = & aws ec2 wait instance-terminated --region $script:AwsRegion --instance-ids $iid 2>&1
                    }
                }
                "gcp" {
                    $null = & gcloud compute instances delete $Name `
                        --project $script:GcpProject --zone $script:GcpZone --quiet 2>&1
                }
            }
            $ErrorActionPreference = $prevErr2
            Write-Ok "Instance deleted"
            return $false
        }
    }
    return $false
}

# ── Azure deployment ──────────────────────────────────────────────────────────
function Invoke-AzureDeployTester {
    Invoke-AzureCreateVm -label "tester" -rg $script:AzureTesterRg -vm $script:AzureTesterVm `
        -size $script:AzureTesterSize -osType $script:AzureTesterOs
    if ($script:AzureAutoShutdown -eq "yes") {
        Invoke-AzureAutoShutdown $script:AzureTesterVm $script:AzureTesterRg
    }
    Invoke-WaitForSsh -ip $script:AzureTesterIp -user "azureuser" -label "tester instance"
    Invoke-RemoteInstallBinary -binary "networker-tester" -ip $script:AzureTesterIp -user "azureuser"
}

function Invoke-AzureDeployEndpoint {
    Invoke-AzureCreateVm -label "endpoint" -rg $script:AzureEndpointRg -vm $script:AzureEndpointVm `
        -size $script:AzureEndpointSize -osType $script:AzureEndpointOs
    Invoke-AzureOpenPorts $script:AzureEndpointRg $script:AzureEndpointVm
    if ($script:AzureAutoShutdown -eq "yes") {
        Invoke-AzureAutoShutdown $script:AzureEndpointVm $script:AzureEndpointRg
    }
    Invoke-WaitForSsh -ip $script:AzureEndpointIp -user "azureuser" -label "endpoint instance"
    Invoke-RemoteInstallBinary -binary "networker-endpoint" -ip $script:AzureEndpointIp -user "azureuser"
    Invoke-RemoteCreateEndpointService $script:AzureEndpointIp "azureuser"
    Invoke-RemoteVerifyHealth $script:AzureEndpointIp
    Invoke-GenerateConfig $script:AzureEndpointIp
}

function Invoke-AzureCreateVm {
    param($label, $rg, $vm, $size, $osType)
    Invoke-NextStep "Create Azure VM for $label ($vm in $($script:AzureRegion))"

    # Check existence
    $name = if ($label -eq "tester") { $script:AzureTesterVm } else { $script:AzureEndpointVm }
    $reused = Invoke-VmExistsCheck -Provider "azure" -Label $label -Name $name
    if ($reused) { return }
    # Re-read name in case it was changed
    $vm = if ($label -eq "tester") { $script:AzureTesterVm } else { $script:AzureEndpointVm }

    Write-Info "Creating resource group '$rg' in $($script:AzureRegion)..."
    & az group create --name $rg --location $script:AzureRegion --output none
    Write-Ok "Resource group: $rg"

    $image   = if ($osType -eq "windows") { "Win2022Datacenter" } else { "Ubuntu2204" }
    $osLabel = if ($osType -eq "windows") { "Windows Server 2022" } else { "Ubuntu 22.04 LTS" }

    $authOpts = @("--generate-ssh-keys")
    if ($osType -eq "windows") {
        $winPass = "Nwk" + (-join ((65..90) + (97..122) + (48..57) | Get-Random -Count 12 | ForEach-Object {[char]$_})) + "!1"
        $authOpts = @("--admin-password", $winPass)
    }

    Write-Info "Creating $osLabel VM '$vm' ($size)..."
    Write-Dim "This typically takes 1-2 minutes..."
    Write-Host ""

    $ip = & az vm create `
        --resource-group $rg `
        --name $vm `
        --image $image `
        --size $size `
        --admin-username azureuser `
        @authOpts `
        --only-show-errors `
        --output tsv `
        --query publicIpAddress

    if (-not $ip) {
        Write-Err "Failed to retrieve VM public IP."
        exit 1
    }

    if ($label -eq "tester") { $script:AzureTesterIp = $ip }
    else { $script:AzureEndpointIp = $ip }
    Write-Ok "VM created ($osLabel) -- Public IP: $ip"

    if ($osType -eq "windows" -and $winPass) {
        Write-Host ""
        Write-Info "Windows credentials:"
        Write-Host "    User:     azureuser"
        Write-Host "    Password: $winPass"
        Write-Host "    RDP:      mstsc /v:$ip"
        Write-Host ""
    }
}

function Invoke-AzureOpenPorts ($rg, $vm) {
    Invoke-NextStep "Open endpoint ports on Azure NSG"
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $portOut = & az vm open-port --resource-group $rg --name $vm `
        --port "80,443,8080,8443" --priority 1100 --output none 2>&1
    $ErrorActionPreference = $prevErr
    if ($LASTEXITCODE -ne 0) {
        $portText = ($portOut | Out-String)
        if ($portText -match "SecurityRuleConflict|already exists") {
            Write-Ok "Ports already open (existing NSG rule)"
        } else {
            Write-Warn "Port open command returned an error (may already be configured)"
            Write-Dim ($portText.Trim())
        }
    } else {
        Write-Ok "Ports opened: 80, 443, 8080, 8443"
    }
}

function Invoke-AzureAutoShutdown ($vm, $rg) {
    Invoke-NextStep "Set Azure auto-shutdown"
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & az vm auto-shutdown --resource-group $rg --name $vm `
        --time "0400" --location $script:AzureRegion --output none 2>&1
    $ErrorActionPreference = $prevErr
    Write-Ok "Auto-shutdown: 04:00 UTC (11 PM EST) daily"
}

# ── AWS deployment ────────────────────────────────────────────────────────────
function Invoke-AwsFindUbuntuAmi {
    Write-Info "Looking up latest Ubuntu 22.04 AMI..."
    $script:AwsAmiId = (& aws ec2 describe-images `
        --region $script:AwsRegion `
        --owners 099720109477 `
        --filters "Name=name,Values=ubuntu/images/hvm-ssd/ubuntu-jammy-22.04-amd64-server-*" `
                  "Name=state,Values=available" `
        --query "sort_by(Images, &CreationDate)[-1].ImageId" `
        --output text 2>$null) -join ""
    if (-not $script:AwsAmiId -or $script:AwsAmiId -eq "None") {
        Write-Err "Failed to find Ubuntu 22.04 AMI in $($script:AwsRegion)."
        exit 1
    }
    Write-Ok "AMI: $($script:AwsAmiId)"
}

function Invoke-AwsCreateSecurityGroup ($label) {
    $sgName = "networker-$label-sg"
    Write-Info "Creating security group '$sgName'..."
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"

    $sgId = (& aws ec2 describe-security-groups `
        --region $script:AwsRegion `
        --group-names $sgName `
        --query "SecurityGroups[0].GroupId" `
        --output text 2>$null) -join ""

    if (-not $sgId -or $sgId -eq "None") {
        $sgId = (& aws ec2 create-security-group `
            --region $script:AwsRegion `
            --group-name $sgName `
            --description "Networker $label ports" `
            --query "GroupId" `
            --output text 2>$null) -join ""

        # Open SSH
        & aws ec2 authorize-security-group-ingress `
            --region $script:AwsRegion --group-id $sgId `
            --protocol tcp --port 22 --cidr 0.0.0.0/0 --output text >$null 2>&1

        if ($label -eq "endpoint") {
            foreach ($port in @(80, 443, 8080, 8443)) {
                & aws ec2 authorize-security-group-ingress `
                    --region $script:AwsRegion --group-id $sgId `
                    --protocol tcp --port $port --cidr 0.0.0.0/0 --output text >$null 2>&1
            }
            foreach ($port in @(8443, 9998, 9999)) {
                & aws ec2 authorize-security-group-ingress `
                    --region $script:AwsRegion --group-id $sgId `
                    --protocol udp --port $port --cidr 0.0.0.0/0 --output text >$null 2>&1
            }
        }
    }
    $ErrorActionPreference = $prevErr
    Write-Ok "Security group: $sgId"
    return $sgId
}

function Invoke-AwsEnsureKeypair {
    # Check if SSH key exists locally
    $keyFile = $null
    foreach ($kf in @("$env:USERPROFILE\.ssh\id_ed25519.pub", "$env:USERPROFILE\.ssh\id_rsa.pub")) {
        if (Test-Path $kf) { $keyFile = $kf; break }
    }
    if (-not $keyFile) { return }  # No local key — EC2 will use password/SSM

    Write-Info "Ensuring SSH keypair 'networker-keypair'..."
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    # Delete existing (may be stale)
    & aws ec2 delete-key-pair --region $script:AwsRegion --key-name networker-keypair --output text >$null 2>&1
    # Import current key
    & aws ec2 import-key-pair --region $script:AwsRegion `
        --key-name networker-keypair `
        --public-key-material "fileb://$keyFile" --output text >$null 2>&1
    $ErrorActionPreference = $prevErr
    Write-Ok "SSH keypair imported"
}

function Invoke-AwsLaunchInstance {
    param($label, $instanceType, $nameTag, $sgId)
    Invoke-NextStep "Create AWS EC2 instance for $label ($nameTag, $($script:AwsRegion))"

    # Check existence
    $reused = Invoke-VmExistsCheck -Provider "aws" -Label $label -Name $nameTag
    if ($reused) { return }
    $nameTag = if ($label -eq "tester") { $script:AwsTesterName } else { $script:AwsEndpointName }

    Write-Info "Launching EC2 instance ($instanceType, $nameTag)..."
    Write-Dim "This typically takes 1-2 minutes..."
    Write-Host ""

    $keyOpt = @()
    foreach ($kf in @("$env:USERPROFILE\.ssh\id_ed25519.pub", "$env:USERPROFILE\.ssh\id_rsa.pub")) {
        if (Test-Path $kf) { $keyOpt = @("--key-name", "networker-keypair"); break }
    }

    $instanceId = (& aws ec2 run-instances `
        --region $script:AwsRegion `
        --image-id $script:AwsAmiId `
        --instance-type $instanceType `
        @keyOpt `
        --security-group-ids $sgId `
        --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=$nameTag}]" `
        --query "Instances[0].InstanceId" `
        --output text 2>$null) -join ""

    if (-not $instanceId -or $instanceId -eq "None") {
        Write-Err "Failed to launch EC2 instance."
        exit 1
    }
    if ($label -eq "tester") { $script:AwsTesterInstanceId = $instanceId }
    else { $script:AwsEndpointInstanceId = $instanceId }
    Write-Ok "Instance launched: $instanceId"

    Write-Info "Waiting for instance to reach 'running' state..."
    & aws ec2 wait instance-running --region $script:AwsRegion --instance-ids $instanceId

    $publicIp = (& aws ec2 describe-instances `
        --region $script:AwsRegion --instance-ids $instanceId `
        --query "Reservations[0].Instances[0].PublicIpAddress" `
        --output text 2>$null) -join ""

    if (-not $publicIp -or $publicIp -eq "None") {
        Write-Err "Instance has no public IP."
        exit 1
    }
    if ($label -eq "tester") { $script:AwsTesterIp = $publicIp }
    else { $script:AwsEndpointIp = $publicIp }
    Write-Ok "Instance running -- Public IP: $publicIp"
}

function Invoke-AwsDeployTester {
    Invoke-AwsEnsureKeypair
    Invoke-AwsFindUbuntuAmi
    $sgId = Invoke-AwsCreateSecurityGroup "tester"
    Invoke-AwsLaunchInstance -label "tester" -instanceType $script:AwsTesterType -nameTag $script:AwsTesterName -sgId $sgId
    Invoke-WaitForSsh -ip $script:AwsTesterIp -user "ubuntu" -label "tester instance"
    if ($script:AwsAutoShutdown -eq "yes") {
        Invoke-RemoteAutoShutdownCron $script:AwsTesterIp "ubuntu"
    }
    Invoke-RemoteInstallBinary -binary "networker-tester" -ip $script:AwsTesterIp -user "ubuntu"
}

function Invoke-AwsDeployEndpoint {
    if (-not $script:AwsAmiId) {
        Invoke-AwsEnsureKeypair
        Invoke-AwsFindUbuntuAmi
    }
    $sgId = Invoke-AwsCreateSecurityGroup "endpoint"
    Invoke-AwsLaunchInstance -label "endpoint" -instanceType $script:AwsEndpointType -nameTag $script:AwsEndpointName -sgId $sgId
    Invoke-WaitForSsh -ip $script:AwsEndpointIp -user "ubuntu" -label "endpoint instance"
    if ($script:AwsAutoShutdown -eq "yes") {
        Invoke-RemoteAutoShutdownCron $script:AwsEndpointIp "ubuntu"
    }
    Invoke-RemoteInstallBinary -binary "networker-endpoint" -ip $script:AwsEndpointIp -user "ubuntu"
    Invoke-RemoteCreateEndpointService $script:AwsEndpointIp "ubuntu"
    Invoke-RemoteVerifyHealth $script:AwsEndpointIp
    Invoke-GenerateConfig $script:AwsEndpointIp
}

# ── GCP deployment ────────────────────────────────────────────────────────────
function Invoke-GcpCheckPrereqs {
    Invoke-NextStep "Check GCP prerequisites"

    Write-Ok "gcloud CLI found"

    # Ensure logged in
    if (-not $script:GcpLoggedIn) {
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $acct = (& gcloud config get-value account 2>$null) -join ""
        $ErrorActionPreference = $prevErr
        if ($acct -and $acct -ne "(unset)") {
            $script:GcpLoggedIn = $true
        }
    }

    # Check GOOGLE_APPLICATION_CREDENTIALS (service account key file)
    if (-not $script:GcpLoggedIn -and $env:GOOGLE_APPLICATION_CREDENTIALS -and (Test-Path $env:GOOGLE_APPLICATION_CREDENTIALS)) {
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & gcloud auth activate-service-account --key-file $env:GOOGLE_APPLICATION_CREDENTIALS --quiet 2>$null
        $acct = (& gcloud config get-value account 2>$null) -join ""
        $ErrorActionPreference = $prevErr
        if ($acct -and $acct -ne "(unset)") {
            $script:GcpLoggedIn = $true
            Write-Ok "GCP credentials found  ($acct)"
        }
    }

    if (-not $script:GcpLoggedIn) {
        Write-Warn "Not logged in to GCP."
        & gcloud auth login --no-launch-browser
        $acct = (& gcloud config get-value account 2>$null) -join ""
        if ($acct -and $acct -ne "(unset)") {
            $script:GcpLoggedIn = $true
        } else {
            Write-Err "GCP login failed."
            exit 1
        }
    }

    Invoke-GcpResolveProject

    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $acct = (& gcloud config get-value account 2>$null) -join ""
    $ErrorActionPreference = $prevErr
    Write-Ok "Account: $acct  (project: $($script:GcpProject))"

    # Enable Compute Engine API
    Write-Info "Checking Compute Engine API..."
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $apiStatus = (& gcloud services list --enabled `
        --filter "config.name=compute.googleapis.com" `
        --format "value(config.name)" `
        --project $script:GcpProject 2>$null) -join ""
    $ErrorActionPreference = $prevErr

    if ($apiStatus -ne "compute.googleapis.com") {
        Write-Warn "Compute Engine API is not enabled."
        if (Invoke-AskYN "Enable Compute Engine API now?" "y") {
            & gcloud services enable compute.googleapis.com --project $script:GcpProject
            Write-Ok "Compute Engine API enabled"
        } else {
            Write-Err "Compute Engine API is required."
            exit 1
        }
    } else {
        Write-Ok "Compute Engine API enabled"
    }
}

function Invoke-GcpCreateFirewallRule {
    $ruleName = "networker-endpoint-allow"
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $null = & gcloud compute firewall-rules describe $ruleName --project $script:GcpProject 2>&1
    $ruleExists = ($LASTEXITCODE -eq 0)
    $ErrorActionPreference = $prevErr

    if ($ruleExists) {
        Write-Ok "Firewall rule '$ruleName' already exists -- reusing"
        return
    }

    Write-Info "Creating firewall rule '$ruleName'..."
    & gcloud compute firewall-rules create $ruleName `
        --project $script:GcpProject `
        --direction INGRESS `
        --action ALLOW `
        --rules "tcp:22,tcp:80,tcp:443,tcp:3389,tcp:8080,tcp:8443,udp:8443,udp:9998,udp:9999" `
        --source-ranges "0.0.0.0/0" `
        --target-tags networker-endpoint `
        --quiet
    Write-Ok "Firewall rule created"
}

function Invoke-GcpCreateInstance {
    param($label, $name, $machineType)
    Invoke-NextStep "Create GCE instance for $label ($name in $($script:GcpZone))"

    $reused = Invoke-VmExistsCheck -Provider "gcp" -Label $label -Name $name
    if ($reused) { return }
    $name = if ($label -eq "tester") { $script:GcpTesterName } else { $script:GcpEndpointName }

    $tagsOpt = @()
    if ($label -eq "endpoint") { $tagsOpt = @("--tags=networker-endpoint") }

    # Determine OS image
    $osType = if ($label -eq "tester") { $script:GcpTesterOs } else { $script:GcpEndpointOs }
    if ($osType -eq "windows") {
        $imageFamily = "windows-2022"; $imageProject = "windows-cloud"; $osLabel = "Windows Server 2022"
    } else {
        $imageFamily = "ubuntu-2204-lts"; $imageProject = "ubuntu-os-cloud"; $osLabel = "Ubuntu 22.04"
    }

    Write-Info "Creating $osLabel VM '$name' ($machineType)..."
    Write-Dim "This typically takes 1-2 minutes..."
    Write-Host ""

    & gcloud compute instances create $name `
        --project $script:GcpProject `
        --zone $script:GcpZone `
        --machine-type $machineType `
        --image-family $imageFamily `
        --image-project $imageProject `
        @tagsOpt `
        --quiet

    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $ip = (& gcloud compute instances describe $name `
        --project $script:GcpProject `
        --zone $script:GcpZone `
        --format "get(networkInterfaces[0].accessConfigs[0].natIP)" 2>$null) -join ""
    $ErrorActionPreference = $prevErr

    if (-not $ip) {
        Write-Err "Failed to retrieve instance public IP."
        exit 1
    }

    if ($label -eq "tester") { $script:GcpTesterIp = $ip }
    else { $script:GcpEndpointIp = $ip }
    Write-Ok "Instance created -- Public IP: $ip"
}

function Invoke-GcpWaitForSsh ($name, $label) {
    Write-Info "Waiting for SSH access to $label..."
    $attempt = 0
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    while ($attempt -lt 30) {
        $null = & gcloud compute ssh $name `
            --project $script:GcpProject `
            --zone $script:GcpZone `
            --command "echo ok" `
            --quiet `
            --ssh-flag="-o ConnectTimeout=5" `
            --ssh-flag="-o StrictHostKeyChecking=no" 2>&1
        if ($LASTEXITCODE -eq 0) {
            $ErrorActionPreference = $prevErr
            Write-Ok "SSH available on $label"
            return
        }
        $attempt++
        Start-Sleep -Seconds 5
    }
    $ErrorActionPreference = $prevErr
    Write-Warn "SSH not available after 150s -- continuing anyway"
}

function Invoke-GcpSshRun ($name, $command) {
    & gcloud compute ssh $name `
        --project $script:GcpProject `
        --zone $script:GcpZone `
        --quiet `
        --ssh-flag="-o StrictHostKeyChecking=no" `
        --command $command
}

# ── GCP Windows VM helpers ────────────────────────────────────────────────────

function Invoke-GcpWaitForWindowsVm ($name, $label) {
    Write-Info "Waiting for $label Windows VM to be ready (3-5 minutes)..."
    for ($i = 0; $i -lt 40; $i++) {
        $null = & gcloud compute ssh $name `
            --project $script:GcpProject `
            --zone $script:GcpZone `
            --command "echo ready" `
            --quiet `
            --ssh-flag="-o ConnectTimeout=10" `
            --ssh-flag="-o StrictHostKeyChecking=no" 2>$null
        if ($LASTEXITCODE -eq 0) {
            Write-Host ""
            Write-Ok "Windows VM ready (SSH available)"
            return
        }
        Write-Host -NoNewline "."
        Start-Sleep -Seconds 10
    }
    Write-Host ""
    Write-Warn "Windows VM not responding via SSH after ~7 minutes."
    Write-Info "Try: gcloud compute reset-windows-password $name --zone $($script:GcpZone)"
}

function Invoke-GcpResetWindowsPassword ($name, $label) {
    Write-Info "Setting Windows password for $label..."
    $creds = & gcloud compute reset-windows-password $name `
        --project $script:GcpProject `
        --zone $script:GcpZone `
        --user networker `
        --quiet 2>&1
    $ip   = ($creds | Where-Object { $_ -match '^ip_address:' }) -replace '^ip_address:\s*',''
    $user = ($creds | Where-Object { $_ -match '^username:' })   -replace '^username:\s*',''
    $pass = ($creds | Where-Object { $_ -match '^password:' })   -replace '^password:\s*',''
    if ($pass) {
        Write-Host ""
        Write-Info "Windows credentials for ${label}:"
        Write-Host "    User:     $user"
        Write-Host "    Password: $pass"
        Write-Host "    RDP:      mstsc /v:$ip"
        Write-Host ""
    } else {
        Write-Warn "Could not retrieve Windows password automatically."
        Write-Info "Run: gcloud compute reset-windows-password $name --zone $($script:GcpZone)"
    }
}

function Invoke-GcpWinInstallBinary ($binary, $name) {
    $archive = "${binary}-x86_64-pc-windows-msvc.zip"
    $ver = $script:NetworkerVersion
    if (-not $ver) { $ver = "latest" }
    $url = "$($script:RepoHttps)/releases/download/${ver}/${archive}"

    Invoke-NextStep "Install ${binary}.exe on GCE Windows VM"
    Write-Info "Installing ${binary}.exe on Windows VM..."
    & gcloud compute ssh $name `
        --project $script:GcpProject `
        --zone $script:GcpZone `
        --quiet `
        --ssh-flag="-o StrictHostKeyChecking=no" `
        --command "powershell -Command `"`$ErrorActionPreference='Stop'; New-Item -ItemType Directory -Force -Path C:\networker-tmp | Out-Null; New-Item -ItemType Directory -Force -Path C:\networker | Out-Null; Invoke-WebRequest -Uri '$url' -OutFile 'C:\networker-tmp\$archive' -UseBasicParsing; Expand-Archive -Path 'C:\networker-tmp\$archive' -DestinationPath 'C:\networker' -Force; Remove-Item -Recurse -Force C:\networker-tmp; `$mp=[System.Environment]::GetEnvironmentVariable('Path','Machine'); if(`$mp -notlike '*C:\networker*'){[System.Environment]::SetEnvironmentVariable('Path',`"`$mp;C:\networker`",'Machine')}; & 'C:\networker\${binary}.exe' --version`""
    if ($LASTEXITCODE -eq 0) { Write-Ok "${binary}.exe installed on GCE Windows VM" }
    else { Write-Warn "${binary}.exe may not have installed correctly -- check the VM" }
}

function Invoke-GcpWinCreateEndpointService ($name) {
    Invoke-NextStep "Create networker-endpoint Windows service (GCP)"
    Write-Info "Creating Windows Service and opening firewall ports..."
    & gcloud compute ssh $name `
        --project $script:GcpProject `
        --zone $script:GcpZone `
        --quiet `
        --ssh-flag="-o StrictHostKeyChecking=no" `
        --command "powershell -Command `"`$ErrorActionPreference='Continue'; sc.exe create networker-endpoint binPath='C:\networker\networker-endpoint.exe' start=auto; sc.exe description networker-endpoint 'Networker Endpoint diagnostics server'; sc.exe start networker-endpoint; netsh advfirewall firewall add rule name='Networker-HTTP' protocol=TCP dir=in action=allow localport=8080; netsh advfirewall firewall add rule name='Networker-HTTPS' protocol=TCP dir=in action=allow localport=8443; netsh advfirewall firewall add rule name='Networker-UDP' protocol=UDP dir=in action=allow localport='8443,9998,9999'`""
    Write-Ok "networker-endpoint service created on GCE Windows VM"
}

function Invoke-GcpWinSetAutoShutdown ($name, $label) {
    if ($script:GcpAutoShutdown -ne "yes") { return }
    Invoke-NextStep "Set auto-shutdown for $label (04:00 UTC)"
    & gcloud compute ssh $name `
        --project $script:GcpProject `
        --zone $script:GcpZone `
        --quiet `
        --ssh-flag="-o StrictHostKeyChecking=no" `
        --command "powershell -Command `"`$action = New-ScheduledTaskAction -Execute 'shutdown.exe' -Argument '/s /t 60 /f'; `$trigger = New-ScheduledTaskTrigger -Daily -At '04:00'; Register-ScheduledTask -TaskName 'NetworkerAutoShutdown' -Action `$action -Trigger `$trigger -User 'SYSTEM' -RunLevel Highest -Force`""
    if ($LASTEXITCODE -eq 0) { Write-Ok "Auto-shutdown task installed: 04:00 UTC daily" }
    else { Write-Warn "Could not install auto-shutdown task (non-critical)" }
}

# ── GCP deploy orchestration ─────────────────────────────────────────────────

function Invoke-GcpDeployTester {
    Invoke-GcpCheckPrereqs
    Invoke-GcpCreateInstance -label "tester" -name $script:GcpTesterName -machineType $script:GcpTesterMachineType

    if ($script:GcpTesterOs -eq "windows") {
        Invoke-GcpWaitForWindowsVm $script:GcpTesterName "tester instance"
        Invoke-GcpResetWindowsPassword $script:GcpTesterName "tester"
        Invoke-GcpWinSetAutoShutdown $script:GcpTesterName "tester instance"
        Invoke-GcpWinInstallBinary "networker-tester" $script:GcpTesterName
    } else {
        Invoke-GcpWaitForSsh $script:GcpTesterName "tester instance"
        if ($script:GcpAutoShutdown -eq "yes") {
            Invoke-NextStep "Set auto-shutdown cron for tester"
            Invoke-GcpSshRun $script:GcpTesterName "(crontab -l 2>/dev/null; echo '0 4 * * * /sbin/shutdown -h now') | crontab -"
            Write-Ok "Auto-shutdown cron installed"
        }
        Invoke-NextStep "Install networker-tester on GCE instance"
        Invoke-GcpInstallBinary "networker-tester" $script:GcpTesterName
    }
}

function Invoke-GcpDeployEndpoint {
    Invoke-GcpCheckPrereqs
    Invoke-GcpCreateFirewallRule
    Invoke-GcpCreateInstance -label "endpoint" -name $script:GcpEndpointName -machineType $script:GcpEndpointMachineType

    if ($script:GcpEndpointOs -eq "windows") {
        Invoke-GcpWaitForWindowsVm $script:GcpEndpointName "endpoint instance"
        Invoke-GcpResetWindowsPassword $script:GcpEndpointName "endpoint"
        Invoke-GcpWinSetAutoShutdown $script:GcpEndpointName "endpoint instance"
        Invoke-GcpWinInstallBinary "networker-endpoint" $script:GcpEndpointName
        Invoke-GcpWinCreateEndpointService $script:GcpEndpointName
    } else {
        Invoke-GcpWaitForSsh $script:GcpEndpointName "endpoint instance"
        if ($script:GcpAutoShutdown -eq "yes") {
            Invoke-NextStep "Set auto-shutdown cron for endpoint"
            Invoke-GcpSshRun $script:GcpEndpointName "(crontab -l 2>/dev/null; echo '0 4 * * * /sbin/shutdown -h now') | crontab -"
            Write-Ok "Auto-shutdown cron installed"
        }
        Invoke-NextStep "Install networker-endpoint on GCE instance"
        Invoke-GcpInstallBinary "networker-endpoint" $script:GcpEndpointName
        Invoke-NextStep "Create networker-endpoint service (GCP)"
        Invoke-GcpSshRun $script:GcpEndpointName @"
sudo useradd --system --no-create-home --shell /usr/sbin/nologin networker 2>/dev/null || true
sudo tee /etc/systemd/system/networker-endpoint.service > /dev/null <<'UNIT'
[Unit]
Description=Networker Endpoint
After=network.target
[Service]
User=networker
ExecStart=/usr/local/bin/networker-endpoint
Restart=always
RestartSec=5
Environment=RUST_LOG=info
[Install]
WantedBy=multi-user.target
UNIT
sudo systemctl daemon-reload
sudo systemctl enable networker-endpoint
sudo systemctl start networker-endpoint
if command -v iptables &>/dev/null; then
    sudo iptables -t nat -C PREROUTING -p tcp --dport 80 -j REDIRECT --to-port 8080 2>/dev/null || sudo iptables -t nat -A PREROUTING -p tcp --dport 80 -j REDIRECT --to-port 8080
    sudo iptables -t nat -C PREROUTING -p tcp --dport 443 -j REDIRECT --to-port 8443 2>/dev/null || sudo iptables -t nat -A PREROUTING -p tcp --dport 443 -j REDIRECT --to-port 8443
fi
"@
        Write-Ok "Endpoint service enabled and started"
    }

    Invoke-NextStep "Verify endpoint health (GCP)"
    Start-Sleep -Seconds 3
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $health = Invoke-GcpSshRun $script:GcpEndpointName "curl -sf http://localhost:8080/health 2>/dev/null"
    $ErrorActionPreference = $prevErr
    if ($health) { Write-Ok "Endpoint healthy" }
    else { Write-Warn "Health check inconclusive -- endpoint may still be starting" }

    Invoke-GenerateConfig $script:GcpEndpointIp
}

function Invoke-GcpInstallBinary ($binary, $name) {
    $component = if ($binary -eq "networker-tester") { "tester" } else { "endpoint" }
    $installerUrl = "https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.sh"

    Write-Info "Downloading installer on instance..."
    Invoke-GcpSshRun $name "curl -fsSL '$installerUrl' -o /tmp/networker-install.sh"

    Write-Info "Running installer on instance ($component)..."
    Invoke-GcpSshRun $name "bash /tmp/networker-install.sh $component -y"
}

# ══════════════════════════════════════════════════════════════════════════════
#  REMOTE HELPERS (SSH-based — for Azure and AWS Linux VMs)
# ══════════════════════════════════════════════════════════════════════════════

function Invoke-WaitForSsh {
    param($ip, $user, $label)
    Invoke-NextStep "Wait for SSH on $label"
    Write-Info "Waiting for SSH access to $label..."
    $attempt = 0
    while ($attempt -lt 30) {
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $null = & ssh -o StrictHostKeyChecking=no -o ConnectTimeout=5 `
            "${user}@${ip}" "echo ok" 2>&1
        $ok = ($LASTEXITCODE -eq 0)
        $ErrorActionPreference = $prevErr
        if ($ok) {
            Write-Ok "SSH available on $label"
            return
        }
        $attempt++
        Start-Sleep -Seconds 5
    }
    Write-Warn "SSH not available after 150s -- continuing anyway"
}

function Invoke-RemoteInstallBinary {
    param($binary, $ip, $user)
    Invoke-NextStep "Install $binary on remote VM"

    $component = if ($binary -eq "networker-tester") { "tester" } else { "endpoint" }
    $installerUrl = "https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.sh"

    Write-Info "Downloading and running installer on VM..."
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & ssh -o StrictHostKeyChecking=no "${user}@${ip}" `
        "curl -fsSL '${installerUrl}' -o /tmp/networker-install.sh && bash /tmp/networker-install.sh ${component} -y"
    $ErrorActionPreference = $prevErr

    # Verify
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $ver = (& ssh -o StrictHostKeyChecking=no "${user}@${ip}" `
        "/usr/local/bin/${binary} --version 2>/dev/null || ~/.cargo/bin/${binary} --version 2>/dev/null" 2>$null) -join ""
    $ErrorActionPreference = $prevErr
    if ($ver) { Write-Ok "$binary installed on VM  ($ver)" }
    else { Write-Warn "$binary install may have failed -- check VM manually" }
}

function Invoke-RemoteCreateEndpointService ($ip, $user) {
    Invoke-NextStep "Create networker-endpoint service"
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $script = @"
sudo useradd --system --no-create-home --shell /usr/sbin/nologin networker 2>/dev/null || true
sudo tee /etc/systemd/system/networker-endpoint.service > /dev/null <<'UNIT'
[Unit]
Description=Networker Endpoint
After=network.target
[Service]
User=networker
ExecStart=/usr/local/bin/networker-endpoint
Restart=always
RestartSec=5
Environment=RUST_LOG=info
[Install]
WantedBy=multi-user.target
UNIT
sudo systemctl daemon-reload
sudo systemctl enable networker-endpoint
sudo systemctl start networker-endpoint
if command -v iptables &>/dev/null; then
    sudo iptables -t nat -C PREROUTING -p tcp --dport 80 -j REDIRECT --to-port 8080 2>/dev/null || sudo iptables -t nat -A PREROUTING -p tcp --dport 80 -j REDIRECT --to-port 8080
    sudo iptables -t nat -C PREROUTING -p tcp --dport 443 -j REDIRECT --to-port 8443 2>/dev/null || sudo iptables -t nat -A PREROUTING -p tcp --dport 443 -j REDIRECT --to-port 8443
fi
"@
    # Convert CRLF to LF to avoid errors on Linux
    $script = $script -replace "`r`n", "`n"
    $script | & ssh -o StrictHostKeyChecking=no "${user}@${ip}" "bash -s"
    $ErrorActionPreference = $prevErr
    Start-Sleep -Seconds 2
    Write-Ok "networker-endpoint service enabled and started"
}

function Invoke-RemoteVerifyHealth ($ip) {
    Invoke-NextStep "Verify endpoint health"
    Start-Sleep -Seconds 3
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $health = Invoke-WebRequest -Uri "http://${ip}:8080/health" -UseBasicParsing -TimeoutSec 10
        Write-Ok "Endpoint healthy (HTTP $($health.StatusCode))"
    } catch {
        Write-Warn "Health check failed -- endpoint may still be starting"
    }
    $ErrorActionPreference = $prevErr
}

function Invoke-RemoteAutoShutdownCron ($ip, $user) {
    Invoke-NextStep "Set auto-shutdown cron"
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & ssh -o StrictHostKeyChecking=no "${user}@${ip}" `
        "(crontab -l 2>/dev/null; echo '0 4 * * * /sbin/shutdown -h now') | crontab -"
    $ErrorActionPreference = $prevErr
    Write-Ok "Auto-shutdown cron installed: 04:00 UTC (11 PM EST) daily"
}

function Invoke-GenerateConfig ($endpointIp) {
    Invoke-NextStep "Generate test config"
    $configPath = Join-Path $env:USERPROFILE "networker-cloud.json"
    $config = @{
        target = "http://${endpointIp}:8080/health"
        modes  = @("http1", "http2", "tcp", "tls", "dns")
        runs   = 5
    } | ConvertTo-Json -Depth 3
    [System.IO.File]::WriteAllText($configPath, $config, [System.Text.UTF8Encoding]::new($false))
    $script:ConfigFilePath = $configPath
    Write-Ok "Config saved: $configPath"
}

# ── Completion summary ─────────────────────────────────────────────────────────
function Show-Completion {
    Write-Host ""
    Write-Host ("=" * 58) -ForegroundColor Green
    Write-Host "  Installation complete!" -ForegroundColor Green
    Write-Host ("=" * 58) -ForegroundColor Green
    Write-Host ""

    $doLocalTester   = $script:DoInstallTester   -and -not $script:DoRemoteTester
    $doLocalEndpoint = $script:DoInstallEndpoint -and -not $script:DoRemoteEndpoint

    if (($doLocalTester -or $doLocalEndpoint) -and $env:PATH -notlike "*$CargoBin*") {
        Write-Warn "$CargoBin is not in PATH for this session."
        Write-Host ""
        Write-Host ('  Run now:  $env:PATH = "' + $CargoBin + ';$env:PATH"')
        Write-Host ""
    }

    if ($doLocalTester) {
        Write-Host "  networker-tester quick start:" -ForegroundColor White
        Write-Host "    networker-tester --help"
        Write-Host "    networker-tester --target http://localhost:8080/health --modes http1 --runs 3"
        Write-Host ""
    }
    if ($doLocalEndpoint) {
        Write-Host "  networker-endpoint quick start:" -ForegroundColor White
        Write-Host "    networker-endpoint"
        Write-Host "    # Listens on :8080 HTTP, :8443 HTTPS/H2/H3, :9998 UDP throughput, :9999 UDP echo"
        Write-Host ""
    }

    # Remote tester summary
    if ($script:DoRemoteTester) {
        $tIp = ""; $tSsh = ""
        switch ($script:TesterLocation) {
            "lan"   { $tIp = $script:LanTesterIp
                      if ($script:LanTesterPort -ne "22") { $tSsh = "ssh -p $($script:LanTesterPort) $($script:LanTesterUser)@$tIp" }
                      else { $tSsh = "ssh $($script:LanTesterUser)@$tIp" } }
            "azure" { $tIp = $script:AzureTesterIp; $tSsh = "ssh azureuser@$tIp" }
            "aws"   { $tIp = $script:AwsTesterIp;   $tSsh = "ssh ubuntu@$tIp" }
            "gcp"   { $tIp = $script:GcpTesterIp;   $tSsh = "gcloud compute ssh $($script:GcpTesterName) --zone $($script:GcpZone)" }
        }
        if ($tIp) {
            $provider = $script:TesterLocation.ToUpper()
            Write-Host "  networker-tester ($provider $tIp):" -ForegroundColor White
            Write-Host "    SSH: $tSsh"
            Write-Host ""
        }
    }

    # Remote endpoint summary
    if ($script:DoRemoteEndpoint) {
        $eIp = ""; $eSsh = ""
        switch ($script:EndpointLocation) {
            "lan"   { $eIp = $script:LanEndpointIp
                      if ($script:LanEndpointPort -ne "22") { $eSsh = "ssh -p $($script:LanEndpointPort) $($script:LanEndpointUser)@$eIp" }
                      else { $eSsh = "ssh $($script:LanEndpointUser)@$eIp" } }
            "azure" { $eIp = $script:AzureEndpointIp; $eSsh = "ssh azureuser@$eIp" }
            "aws"   { $eIp = $script:AwsEndpointIp;   $eSsh = "ssh ubuntu@$eIp" }
            "gcp"   { $eIp = $script:GcpEndpointIp;   $eSsh = "gcloud compute ssh $($script:GcpEndpointName) --zone $($script:GcpZone)" }
        }
        if ($eIp) {
            $provider = $script:EndpointLocation.ToUpper()
            Write-Host "  networker-endpoint ($provider $eIp):" -ForegroundColor White
            Write-Host "    Health: curl http://${eIp}:8080/health"
            Write-Host "    SSH:    $eSsh"
            Write-Host ""
        }
    }

    # Config file
    if ($script:ConfigFilePath) {
        Write-Host "  Test config:  $($script:ConfigFilePath)" -ForegroundColor White
        Write-Host "    networker-tester --config $($script:ConfigFilePath)"
        Write-Host ""
    }

    # Cleanup reminders
    if ($script:TesterLocation -eq "azure" -or $script:EndpointLocation -eq "azure") {
        if ($script:AzureAutoShutdown -eq "yes") {
            Write-Host "  Auto-shutdown configured: Azure VMs will stop at 04:00 UTC daily." -ForegroundColor Green
        } else {
            Write-Warn "Azure VMs are left running -- delete when done to avoid charges!"
        }
        Write-Host ""
        Write-Dim "Delete Azure resources when done:"
        if ($script:TesterLocation -eq "azure") {
            Write-Dim "  az group delete --name $($script:AzureTesterRg) --yes --no-wait"
        }
        if ($script:EndpointLocation -eq "azure") {
            Write-Dim "  az group delete --name $($script:AzureEndpointRg) --yes --no-wait"
        }
        Write-Host ""
    }

    if ($script:TesterLocation -eq "aws" -or $script:EndpointLocation -eq "aws") {
        if ($script:AwsAutoShutdown -eq "yes") {
            Write-Host "  Auto-shutdown configured: AWS instances will stop at 04:00 UTC daily." -ForegroundColor Green
        } else {
            Write-Warn "AWS instances are left running -- terminate when done!"
        }
        Write-Host ""
        Write-Dim "Terminate AWS instances when done:"
        if ($script:TesterLocation -eq "aws" -and $script:AwsTesterInstanceId) {
            Write-Dim "  aws ec2 terminate-instances --region $($script:AwsRegion) --instance-ids $($script:AwsTesterInstanceId)"
        }
        if ($script:EndpointLocation -eq "aws" -and $script:AwsEndpointInstanceId) {
            Write-Dim "  aws ec2 terminate-instances --region $($script:AwsRegion) --instance-ids $($script:AwsEndpointInstanceId)"
        }
        Write-Host ""
    }

    if ($script:TesterLocation -eq "gcp" -or $script:EndpointLocation -eq "gcp") {
        if ($script:GcpAutoShutdown -eq "yes") {
            Write-Host "  Auto-shutdown configured: GCP instances will stop at 04:00 UTC daily." -ForegroundColor Green
        } else {
            Write-Warn "GCP instances are left running -- delete when done!"
        }
        Write-Host ""
        Write-Dim "Delete GCP instances when done:"
        if ($script:TesterLocation -eq "gcp") {
            Write-Dim "  gcloud compute instances delete $($script:GcpTesterName) --zone $($script:GcpZone) --quiet"
        }
        if ($script:EndpointLocation -eq "gcp") {
            Write-Dim "  gcloud compute instances delete $($script:GcpEndpointName) --zone $($script:GcpZone) --quiet"
        }
        Write-Host ""
    }
}

# ══════════════════════════════════════════════════════════════════════════════
#  ENTRY POINT
# ══════════════════════════════════════════════════════════════════════════════
if ($Help) { Show-Help; exit 0 }

if ($Component -and $Component -notin @("tester", "endpoint", "both", "")) {
    Write-Err "Invalid -Component value '$Component'. Use: tester, endpoint, or both."
    exit 1
}

Invoke-DiscoverSystem

Write-Banner
Show-SystemInfo

Invoke-ComponentSelection
Show-Plan
Invoke-MainPrompt

# ── Execute local install steps ──────────────────────────────────────────────
$doLocalTester   = $script:DoInstallTester   -and -not $script:DoRemoteTester
$doLocalEndpoint = $script:DoInstallEndpoint -and -not $script:DoRemoteEndpoint

if ($doLocalTester -or $doLocalEndpoint) {
    if ($script:InstallMethod -eq "release") {
        New-Item -ItemType Directory -Force $CargoBin | Out-Null
        if ($doLocalTester)   { Invoke-DownloadReleaseStep "networker-tester" }
        if ($doLocalEndpoint) { Invoke-DownloadReleaseStep "networker-endpoint" }
    } else {
        if ($script:DoChromiumInstall) { Invoke-ChromeInstallStep }
        if ($script:DoMsvcInstall)     { Invoke-MsvcInstallStep }
        if ($script:DoGitInstall)      { Invoke-GitInstallStep }
        if ($script:DoRustInstall)     { Invoke-RustInstallStep }
        Invoke-EnsureCargoEnv
        if ($doLocalTester)   { Invoke-CargoInstallStep "networker-tester" }
        if ($doLocalEndpoint) { Invoke-CargoInstallStep "networker-endpoint" }
    }
}

# ── Ensure cloud CLIs + options for CLI-flag deployments ─────────────────────
# When using CLI flags (-Azure, -Aws, -Gcp, etc.) the interactive
# Invoke-DeploymentLocationPrompt is skipped, so Ensure*Cli and *Options
# were never called. Run them now (idempotent guards prevent double-prompts).
if ($script:DoRemoteTester) {
    switch ($script:TesterLocation) {
        "azure" { Invoke-EnsureAzureCli; Invoke-AzureOptions "tester" }
        "aws"   { Invoke-EnsureAwsCli;   Invoke-AwsOptions   "tester" }
        "gcp"   { Invoke-EnsureGcpCli;   Invoke-GcpOptions   "tester" }
    }
}
if ($script:DoRemoteEndpoint) {
    switch ($script:EndpointLocation) {
        "azure" { Invoke-EnsureAzureCli; Invoke-AzureOptions "endpoint" }
        "aws"   { Invoke-EnsureAwsCli;   Invoke-AwsOptions   "endpoint" }
        "gcp"   { Invoke-EnsureGcpCli;   Invoke-GcpOptions   "endpoint" }
    }
}

# ── Execute remote deployments ───────────────────────────────────────────────
if ($script:DoRemoteTester) {
    switch ($script:TesterLocation) {
        "lan"   { Invoke-LanDeployTester }
        "azure" { Invoke-AzureDeployTester }
        "aws"   { Invoke-AwsDeployTester }
        "gcp"   { Invoke-GcpDeployTester }
    }
}

if ($script:DoRemoteEndpoint) {
    switch ($script:EndpointLocation) {
        "lan"   { Invoke-LanDeployEndpoint }
        "azure" { Invoke-AzureDeployEndpoint }
        "aws"   { Invoke-AwsDeployEndpoint }
        "gcp"   { Invoke-GcpDeployEndpoint }
    }
}

Show-Completion
