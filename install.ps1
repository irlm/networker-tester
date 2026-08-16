#Requires -Version 5.1
# ──────────────────────────────────────────────────────────────────────────────
# LagHound – Windows interactive installer (rustup-style)
#
# Installs networker-tester and/or networker-endpoint either:
#   locally  – on this machine (release binary download or source compile)
#   remotely – provisioned on a cloud VM (Azure, AWS, and GCP supported)
#
# Two local install modes (auto-detected, or choose in customize flow):
#   release  – download pre-built binary from the latest GitHub release
#              (fast, ~10 s); via gh when authenticated, else the public
#              release URL resolved through the unauthenticated GitHub API
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
# The ConvertTo-SecureString -AsPlainText use in Invoke-EnsureSelfSignedCert
# is intentional: generates a transient password for a local-only self-signed
# PFX that is re-exported to PEM via openssl and then deleted — no secrets at rest.
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '')]
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSReviewUnusedParameter', '')]
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSUseSingularNouns', '')]
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSUseBOMForUnicodeEncodedFile', '')]
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingConvertToSecureStringWithPlainText', '')]
param(
    [string]$Component  = "",
    # -AutoYes is the historical spelling the LAN/remote paths pass; keep it
    # binding (an unknown parameter is a hard error in PowerShell).
    [Alias('AutoYes')]
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
    # -Setup <proxy>  Install ONLY the named reverse-proxy stack (iis, caddy,
    # traefik, haproxy, apache) and exit. Used by install.sh's remote-Windows
    # deploy path (az vm run-command) so the same Invoke-SetupXxx functions
    # are reused for cloud VMs instead of being duplicated as inline PowerShell.
    [string]$Setup      = "",
    # -Fqdn <name>  Optional public DNS name for -Setup iis: adds a hostname
    # (SNI) HTTPS binding next to the IP binding, the same as install.sh's
    # remote Windows deploy passes (AZURE_ENDPOINT_FQDN). Empty = IP binding only.
    [string]$Fqdn       = "",
    # -BenchmarkServer <lang>  Install ONLY the named reference-API language
    # server (apibench then measures the LANGUAGE behind this endpoint) and
    # exit. Windows-viable set: csharp-net48, csharp-net8, csharp-net9,
    # csharp-net10, go, nodejs, python, java. Used by install.sh's remote
    # Windows deploy path (az vm run-command) — the PowerShell mirror of
    # install.sh --benchmark-server. Retargets the endpoint's /api at the
    # language via --api-upstream (endpoint >= 0.28.203 required).
    [string]$BenchmarkServer = "",
    [int]$BenchmarkPort = 8085,
    [switch]$Help
)

$ErrorActionPreference = "Stop"

$RepoHttps     = "https://github.com/irlm/networker-tester"
$RepoGh        = "irlm/networker-tester"
$CargoBin      = Join-Path $env:USERPROFILE ".cargo\bin"
$InstallerVersion = "v0.28.209"  # fallback when gh is unavailable

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
        Write-Host ("      LagHound  " + $script:NetworkerVersion) -ForegroundColor Cyan
    } else {
        Write-Host ("      LagHound Installer") -ForegroundColor Cyan
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
    Write-Host "  release   Download pre-built binary from the GitHub release -- fast (~10 s)"
    Write-Host "            Uses gh when authenticated, else the public release URL"
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
$script:ReleaseViaGh      = $false   # gh release download vs direct asset URL
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

# ── Network helpers ───────────────────────────────────────────────────────────
function Invoke-EnsureTls12 {
    # Windows PowerShell 5.1 defaults to TLS 1.0/1.1 for WebRequest — GitHub
    # and download.microsoft.com require 1.2.
    try {
        [System.Net.ServicePointManager]::SecurityProtocol = `
            [System.Net.ServicePointManager]::SecurityProtocol -bor [System.Net.SecurityProtocolType]::Tls12
    } catch { $script:Tls12Warned = $true }
}

function Get-LatestReleaseTag {
    # Latest release tag via the unauthenticated GitHub API (60 req/h per IP
    # is plenty for an installer). Empty string when offline / rate-limited.
    Invoke-EnsureTls12
    try {
        $rel = Invoke-RestMethod -Uri "https://api.github.com/repos/$RepoGh/releases/latest" `
            -UseBasicParsing -TimeoutSec 15 -Headers @{ "User-Agent" = "networker-install.ps1" }
        if ($rel -and $rel.tag_name) { return [string]$rel.tag_name }
    } catch { Write-Dim "GitHub API not reachable ($($_.Exception.Message)) -- release mode unavailable" }
    return ""
}

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

    # Release mode: available when the platform is in the release matrix AND
    # either gh is authenticated (gh release download) or the latest release
    # tag resolves through the unauthenticated GitHub API (direct download of
    # the public asset with Invoke-WebRequest — the same fallback install.sh
    # has had; a fresh Windows Server VM has neither gh nor Rust, so without
    # it `-Yes -Component endpoint` silently fell into a 10-minute source
    # compile that needs MSVC + winget, absent on Server SKUs).
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
                $script:ReleaseViaGh     = $true
                $script:InstallMethod    = "release"
                $script:NetworkerVersion = (& gh release list --repo $RepoGh `
                    --limit 1 --json tagName --jq ".[0].tagName" 2>$null) -join ""
            }
        }
        if ($target -and -not $script:ReleaseAvailable) {
            $apiTag = Get-LatestReleaseTag
            if ($apiTag) {
                $script:ReleaseTarget    = $target
                $script:ReleaseAvailable = $true
                $script:ReleaseViaGh     = $false
                $script:InstallMethod    = "release"
                $script:NetworkerVersion = $apiTag
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
        if ($script:ReleaseViaGh) {
            Write-Host ("    {0,-22} {1}" -f "gh CLI:", "authenticated v")
        } else {
            Write-Host ("    {0,-22} {1}" -f "Release:", "$($script:NetworkerVersion) via GitHub API (no gh)")
        }
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
        "powershell -ExecutionPolicy Bypass -Command `"& { Invoke-WebRequest -Uri '${installerUrl}' -OutFile C:\networker-install.ps1; & C:\networker-install.ps1 -Component ${component} -Yes }`""
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
    if ($script:ReleaseViaGh) {
        & gh release download --repo $RepoGh --latest `
            --pattern $archive --dir $tmpDir --clobber
        $ok = ($LASTEXITCODE -eq 0)
    } else {
        # No gh: fetch the public asset straight from the release page (the
        # same URL install.sh's remote Windows bootstraps use).
        $ver = if ($script:NetworkerVersion) { $script:NetworkerVersion } else { "latest" }
        $url = if ($ver -eq "latest") { "$RepoHttps/releases/latest/download/$archive" } `
               else { "$RepoHttps/releases/download/$ver/$archive" }
        Invoke-EnsureTls12
        $ok = $false
        try {
            Invoke-WebRequest -Uri $url -OutFile (Join-Path $tmpDir $archive) -UseBasicParsing -TimeoutSec 300
            $ok = Test-Path (Join-Path $tmpDir $archive)
        } catch {
            Write-Warn "download failed: $($_.Exception.Message)"
        }
    }
    $ErrorActionPreference = $prevErr

    if (-not $ok) {
        Write-Host ""
        if ($script:ReleaseViaGh) { Write-Err "gh release download failed." } else { Write-Err "release download failed." }
        Write-Host "  Expected asset: $archive"
        Remove-Item $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
        exit 1
    }

    New-Item -ItemType Directory -Force $CargoBin | Out-Null
    Expand-Archive -Path "$tmpDir\$archive" -DestinationPath $CargoBin -Force
    Remove-Item $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
    Invoke-EnsureVcRuntime

    $installedCmd  = Get-Command $binary -ErrorAction SilentlyContinue
    $installedPath = if ($installedCmd) { $installedCmd.Source } else { "$CargoBin\$binary.exe" }
    $installedVer  = if ($installedCmd) { (& $binary --version 2>&1) } else { "unknown" }
    Write-Host ""
    Write-Ok "$binary installed -> $installedPath  ($installedVer)"
}

function Invoke-EnsureVcRuntime {
    # The release exes are MSVC builds that link vcruntime140.dll dynamically;
    # a fresh Windows Server image does not ship it. Same check the AWS/GCP
    # Windows endpoint bootstraps in install.sh perform before first start.
    if (Test-Path (Join-Path $env:SystemRoot "System32\vcruntime140.dll")) { return }
    Write-Info "Installing Visual C++ Redistributable (vcruntime140.dll missing)..."
    Invoke-EnsureTls12
    $redist = Join-Path $env:TEMP "vc_redist.x64.exe"
    try {
        Invoke-WebRequest -Uri "https://aka.ms/vs/17/release/vc_redist.x64.exe" -OutFile $redist -UseBasicParsing -TimeoutSec 300
        Start-Process -FilePath $redist -ArgumentList @("/install","/quiet","/norestart") -Wait -WindowStyle Hidden
        Remove-Item $redist -Force -ErrorAction SilentlyContinue
        Write-Ok "VC++ Redistributable installed"
    } catch {
        Write-Warn "VC++ Redistributable install failed ($($_.Exception.Message)) -- the binaries may not start until it is present."
    }
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
#  HTTP STACK REVERSE-PROXY SETUP (Windows local)
# ══════════════════════════════════════════════════════════════════════════════
#
# Mirrors Linux install.sh step_setup_{nginx,caddy,apache,haproxy,traefik} and
# IIS on Windows. Each function installs the proxy, writes a minimal config
# that serves the same static test page as nginx/IIS, and registers a Windows
# service so the stack survives reboots.
#
# Port map (aligned with install.sh):
#   nginx    8081 / 8444   (Linux only — handled by install.sh)
#   iis      8082 / 8445   (Windows — Invoke-SetupIIS, twin of install.sh _iis_setup_powershell)
#   caddy    8091 / 8454
#   traefik  8092 / 8455
#   haproxy  8093 / 8456
#   apache   8094 / 8457
#
# All four use $env:ProgramData\networker\<stack> for config + data, generate a
# self-signed cert on first run, and register the service via sc.exe (native)
# falling back to nssm where the binary has no built-in service mode.

$script:NetworkerSiteRoot = Join-Path $env:ProgramData "networker\static"
$script:NetworkerStackDir = Join-Path $env:ProgramData "networker"

function Invoke-EnsureStaticSite {
    # Generate the static test site shared by every HTTP stack. Idempotent.
    if (-not (Test-Path $script:NetworkerSiteRoot)) {
        New-Item -ItemType Directory -Force $script:NetworkerSiteRoot | Out-Null
    }
    $indexPath = Join-Path $script:NetworkerSiteRoot "index.html"
    if (Test-Path $indexPath) { return }

    $epExe = Join-Path $CargoBin "networker-endpoint.exe"
    if (Test-Path $epExe) {
        Write-Info "Generating static test site via networker-endpoint..."
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & $epExe generate-site $script:NetworkerSiteRoot --preset mixed --stack windows 2>&1 | Out-Null
        $ErrorActionPreference = $prevErr
    }
    if (-not (Test-Path $indexPath)) {
        Write-Warn "networker-endpoint generate-site unavailable -- writing fallback test page."
        $html = "<!DOCTYPE html>`n<html><head><title>Networker Page Load Test</title>"
        $html += "<link rel=`"stylesheet`" href=`"style.css`"></head><body>"
        for ($i = 0; $i -lt 50; $i++) { $html += "<img src=`"asset-$i.bin`" width=`"1`" height=`"1`" alt=`"`">" }
        $html += "</body></html>"
        [IO.File]::WriteAllText($indexPath, $html, [Text.Encoding]::UTF8)
        [IO.File]::WriteAllText((Join-Path $script:NetworkerSiteRoot "style.css"), "body{margin:0}", [Text.Encoding]::UTF8)
        $rng = New-Object Random
        $sizes = @(512,2048,4096,8192,16384,32768,65536,102400,204800,409600) * 5
        for ($i = 0; $i -lt $sizes.Count; $i++) {
            $bytes = New-Object byte[] $sizes[$i]
            $rng.NextBytes($bytes)
            [IO.File]::WriteAllBytes((Join-Path $script:NetworkerSiteRoot "asset-$i.bin"), $bytes)
        }
    }
    $healthPath = Join-Path $script:NetworkerSiteRoot "health"
    if (-not (Test-Path $healthPath)) {
        [IO.File]::WriteAllText($healthPath, '{"status":"ok","stack":"windows"}', [Text.Encoding]::UTF8)
    }
}

function Invoke-EnsureSelfSignedCert ($stackDir, $friendlyName) {
    # Emits PEM cert + key into $stackDir. Many Windows builds of Linux-native
    # proxies (caddy/traefik/haproxy/apache) expect PEM — New-SelfSignedCertificate
    # returns PFX, so we export via openssl if available, else fall back to
    # .NET X509 export + a PowerShell-native PEM writer.
    $crtPath = Join-Path $stackDir "networker.crt"
    $keyPath = Join-Path $stackDir "networker.key"
    if ((Test-Path $crtPath) -and (Test-Path $keyPath)) { return @($crtPath, $keyPath) }

    $cert = New-SelfSignedCertificate `
        -DnsName @("localhost", $env:COMPUTERNAME) `
        -CertStoreLocation "Cert:\LocalMachine\My" `
        -NotAfter (Get-Date).AddYears(1) `
        -FriendlyName $friendlyName `
        -KeyExportPolicy Exportable
    $pfxPath = Join-Path $stackDir "networker.pfx"
    $pfxPass = ConvertTo-SecureString -String "networker" -Force -AsPlainText
    Export-PfxCertificate -Cert "Cert:\LocalMachine\My\$($cert.Thumbprint)" -FilePath $pfxPath -Password $pfxPass | Out-Null

    $openssl = Get-Command openssl -ErrorAction SilentlyContinue
    if ($openssl) {
        Start-Process -FilePath $openssl.Source -WindowStyle Hidden -Wait `
            -ArgumentList @("pkcs12","-in",$pfxPath,"-nocerts","-nodes","-out",$keyPath,"-passin","pass:networker")
        Start-Process -FilePath $openssl.Source -WindowStyle Hidden -Wait `
            -ArgumentList @("pkcs12","-in",$pfxPath,"-clcerts","-nokeys","-out",$crtPath,"-passin","pass:networker")
    } else {
        # Fallback: write DER bytes base64-encoded as PEM. Works for the cert;
        # the private key requires openssl — warn and produce cert-only output.
        $raw = $cert.Export([System.Security.Cryptography.X509Certificates.X509ContentType]::Cert)
        $b64 = [Convert]::ToBase64String($raw, [Base64FormattingOptions]::InsertLineBreaks)
        [IO.File]::WriteAllText($crtPath, "-----BEGIN CERTIFICATE-----`n$b64`n-----END CERTIFICATE-----`n")
        Write-Warn "openssl not found -- PEM private key not exported. Install openssl (winget install ShiningLight.OpenSSL) and re-run."
    }
    return @($crtPath, $keyPath)
}

function Invoke-EnsureFirewallRule ($name, $protocol, $ports) {
    $existing = Get-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue
    if ($existing) { return }
    New-NetFirewallRule -DisplayName $name -Direction Inbound -Protocol $protocol `
        -LocalPort $ports -Action Allow -ErrorAction SilentlyContinue | Out-Null
}

function Invoke-EnsureNssm {
    # nssm wraps a plain executable as a Windows service. Used by proxies that
    # have no native --install-service flag (caddy on some versions, traefik).
    $nssmCmd = Get-Command nssm -ErrorAction SilentlyContinue
    if ($nssmCmd) { return $nssmCmd.Source }
    # winget is absent on Windows Server SKUs — calling it there throws before
    # the download fallback below can run (same class as the caddy install;
    # 2026-08-04 diag VM).
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        Write-Info "Installing nssm via winget..."
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & winget install --id NSSM.NSSM -e --source winget `
            --accept-package-agreements --accept-source-agreements 2>&1 | Out-Null
        $ErrorActionPreference = $prevErr

        $machinePath = [System.Environment]::GetEnvironmentVariable("PATH", "Machine")
        $userPath    = [System.Environment]::GetEnvironmentVariable("PATH", "User")
        $env:PATH    = "$machinePath;$userPath"
        $nssmCmd = Get-Command nssm -ErrorAction SilentlyContinue
        if ($nssmCmd) { return $nssmCmd.Source }
    }

    # Fallback: direct download (choco mirror)
    $nssmZip = Join-Path $env:TEMP "nssm.zip"
    $nssmDir = Join-Path $script:NetworkerStackDir "nssm"
    New-Item -ItemType Directory -Force $nssmDir | Out-Null
    Invoke-WebRequest -Uri "https://nssm.cc/release/nssm-2.24.zip" -OutFile $nssmZip -UseBasicParsing
    Expand-Archive -Path $nssmZip -DestinationPath $nssmDir -Force
    $arch = if ($env:PROCESSOR_ARCHITECTURE -eq "x86") { "win32" } else { "win64" }
    $nssmExe = Join-Path $nssmDir "nssm-2.24\$arch\nssm.exe"
    if (Test-Path $nssmExe) { return $nssmExe }
    throw "Failed to install nssm -- cannot register service without it."
}

# ── Caddy (ports 8091 / 8454) ────────────────────────────────────────────────
function Invoke-SetupCaddy {
    Invoke-NextStep "Set up Caddy for HTTP stack comparison (ports 8091/8454)"
    Invoke-EnsureStaticSite

    $stackDir = Join-Path $script:NetworkerStackDir "caddy"
    New-Item -ItemType Directory -Force $stackDir | Out-Null

    $caddyCmd = Get-Command caddy -ErrorAction SilentlyContinue
    # winget does not exist on Windows Server SKUs — calling it there throws
    # under -Setup's strict error mode BEFORE the GitHub fallback below could
    # run, which is why no matrix Caddy cell ever served (2026-08-04 diag VM).
    if (-not $caddyCmd -and (Get-Command winget -ErrorAction SilentlyContinue)) {
        Write-Info "Installing Caddy via winget..."
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & winget install --id CaddyServer.Caddy -e --source winget `
            --accept-package-agreements --accept-source-agreements 2>&1 | Out-Null
        $ErrorActionPreference = $prevErr
        $machinePath = [System.Environment]::GetEnvironmentVariable("PATH", "Machine")
        $env:PATH    = "$machinePath;$env:PATH"
        $caddyCmd = Get-Command caddy -ErrorAction SilentlyContinue
    }
    if (-not $caddyCmd) {
        # Fallback: Caddy's official build service returns the bare exe.
        # (The old GitHub "latest/download/caddy_windows_amd64.zip" shortcut
        # 404s — release assets are version-named — so this fallback had never
        # actually worked; 2026-08-04 diag VM.)
        $arch = if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") { "arm64" } else { "amd64" }
        $caddyExe = Join-Path $stackDir "caddy.exe"
        Write-Info "Downloading Caddy from caddyserver.com..."
        Invoke-WebRequest -UseBasicParsing `
            -Uri "https://caddyserver.com/api/download?os=windows&arch=${arch}" `
            -OutFile $caddyExe -TimeoutSec 120
        if (-not (Test-Path $caddyExe) -or (Get-Item $caddyExe).Length -lt 1MB) {
            Write-Err "Caddy download failed -- skipping."
            throw "caddy install failed: download unavailable"
        }
    } else {
        $caddyExe = $caddyCmd.Source
    }

    $siteRoot = $script:NetworkerSiteRoot -replace '\\','/'
    $caddyfile = Join-Path $stackDir "Caddyfile"
    $cfg = @"
{
    auto_https off
    local_certs
    servers {
        protocols h1 h2 h3
    }
}

# NOTE: Caddy v2 rejects content after '{' on the same line -- one-line
# handle blocks failed `caddy validate` and the service never started
# (identical to the Linux Caddyfile bug fixed in v0.28.133; found on the
# Windows path 2026-08-04). Blocks must be multi-line.
:8091 {
    root * $siteRoot
    file_server
    handle /page* {
        reverse_proxy 127.0.0.1:8080
    }
    handle /asset* {
        reverse_proxy 127.0.0.1:8080
    }
}

:8454 {
    tls internal
    root * $siteRoot
    file_server
    handle /page* {
        reverse_proxy https://127.0.0.1:8443 {
            transport http {
                tls_insecure_skip_verify
            }
        }
    }
    handle /asset* {
        reverse_proxy https://127.0.0.1:8443 {
            transport http {
                tls_insecure_skip_verify
            }
        }
    }
}
"@
    [IO.File]::WriteAllText($caddyfile, $cfg, [Text.Encoding]::UTF8)

    # Register service (Caddy supports --install via nssm wrapper)
    if (Get-Service "networker-caddy" -ErrorAction SilentlyContinue) {
        Start-Process -FilePath "sc.exe" -WindowStyle Hidden -Wait -ArgumentList @("stop","networker-caddy")
        Start-Process -FilePath "sc.exe" -WindowStyle Hidden -Wait -ArgumentList @("delete","networker-caddy")
    }
    $nssm = Invoke-EnsureNssm
    Start-Process -FilePath $nssm -WindowStyle Hidden -Wait -ArgumentList @("install","networker-caddy",$caddyExe,"run","--config",$caddyfile,"--adapter","caddyfile")
    Start-Process -FilePath $nssm -WindowStyle Hidden -Wait -ArgumentList @("set","networker-caddy","AppDirectory",$stackDir)
    Start-Process -FilePath $nssm -WindowStyle Hidden -Wait -ArgumentList @("set","networker-caddy","Start","SERVICE_AUTO_START")
    # Validate BEFORE starting — a config-syntax error otherwise surfaces
    # only as an nssm crash-loop ("Paused" service) with no message.
    # EAP must be Continue around the call: caddy logs INFO to stderr, and
    # under Stop the 2>&1 redirect turns the first info line into a
    # terminating exception (native-stderr ErrorRecord gotcha).
    $prevEAP = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $validation = & $caddyExe validate --config $caddyfile --adapter caddyfile 2>&1
    $ErrorActionPreference = $prevEAP
    if ($LASTEXITCODE -ne 0) {
        Write-Err "Caddyfile failed validation:"
        $validation | Select-Object -Last 3 | ForEach-Object { Write-Err "  $_" }
        throw "caddy install failed: invalid Caddyfile"
    }

    Start-Process -FilePath "sc.exe" -WindowStyle Hidden -Wait -ArgumentList @("start","networker-caddy")

    Invoke-EnsureFirewallRule "Networker-Caddy-HTTP"  "TCP" @(8091)
    Invoke-EnsureFirewallRule "Networker-Caddy-HTTPS" "TCP" @(8454)
    Invoke-EnsureFirewallRule "Networker-Caddy-QUIC"  "UDP" @(8454)

    # Port-serving check — service-active or registered is NOT success.
    Start-Sleep -Seconds 3
    try {
        $null = Invoke-WebRequest -Uri "http://localhost:8091/" -UseBasicParsing -TimeoutSec 8
        Write-Ok "Caddy serving test page on ports 8091 (HTTP) / 8454 (HTTPS+H3)"
    } catch {
        Write-Err "Caddy service registered but port 8091 is not serving."
        throw "caddy install failed: port not serving after start"
    }
}

# ── Traefik (ports 8092 / 8455) ──────────────────────────────────────────────
function Invoke-SetupTraefik {
    Invoke-NextStep "Set up Traefik for HTTP stack comparison (ports 8092/8455)"
    Invoke-EnsureStaticSite

    $stackDir = Join-Path $script:NetworkerStackDir "traefik"
    New-Item -ItemType Directory -Force $stackDir | Out-Null

    $traefikExe = Join-Path $stackDir "traefik.exe"
    if (-not (Test-Path $traefikExe)) {
        # Traefik has no winget package — direct download from GitHub
        Write-Info "Downloading Traefik from GitHub releases..."
        $arch = if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") { "arm64" } else { "amd64" }
        $zipPath = Join-Path $env:TEMP "traefik.zip"
        $url = "https://github.com/traefik/traefik/releases/latest/download/traefik_windows_${arch}.zip"
        try {
            Invoke-WebRequest -Uri $url -OutFile $zipPath -UseBasicParsing
            Expand-Archive -Path $zipPath -DestinationPath $stackDir -Force
        } catch {
            # Fallback to a known-good version if "latest" tag redirect fails
            $fallback = "https://github.com/traefik/traefik/releases/download/v3.2.3/traefik_v3.2.3_windows_${arch}.zip"
            Invoke-WebRequest -Uri $fallback -OutFile $zipPath -UseBasicParsing
            Expand-Archive -Path $zipPath -DestinationPath $stackDir -Force
        }
        if (-not (Test-Path $traefikExe)) {
            Write-Err "Traefik download failed -- skipping."
            return
        }
    }

    $crtKey = Invoke-EnsureSelfSignedCert $stackDir "Networker Traefik Test"
    $crtPath = $crtKey[0] -replace '\\','/'
    $keyPath = $crtKey[1] -replace '\\','/'

    # Traefik doesn't ship a native static-file-server middleware for a folder,
    # so we proxy static paths to the networker-endpoint static mount (same
    # treatment used on Linux for haproxy). Dynamic /page and /asset also
    # forward to the endpoint.
    $staticYaml = @"
# NOTE: declare ONLY the entrypoints this stack owns. Traefik binds every
# declared entrypoint eagerly at startup, so a vestigial ":8091" (which was
# kept here "for doc parity") collides with CADDY's HTTP listener on any host
# running both — traefik then exits instantly and nssm reports SERVICE_PAUSED.
# Proven in CI 2026-08-05: "error while building entryPoint web: listen tcp
# :8091: bind: Only one usage of each socket address".
entryPoints:
  weblocal:
    address: ":8092"
  websecure:
    address: ":8455"
    http3: {}
providers:
  file:
    filename: "$($stackDir -replace '\\','/')/dynamic.yml"
log:
  level: WARN
"@
    [IO.File]::WriteAllText((Join-Path $stackDir "traefik.yml"), $staticYaml, [Text.Encoding]::UTF8)

    $dynamicYaml = @"
http:
  routers:
    static-http:
      rule: "PathPrefix(``/``)"
      entryPoints: [weblocal]
      service: endpoint
    static-https:
      rule: "PathPrefix(``/``)"
      entryPoints: [websecure]
      service: endpoint-tls
      tls: {}
  services:
    endpoint:
      loadBalancer:
        servers:
          - url: "http://127.0.0.1:8080"
    endpoint-tls:
      loadBalancer:
        serversTransport: insecure
        servers:
          - url: "https://127.0.0.1:8443"
  serversTransports:
    insecure:
      insecureSkipVerify: true
tls:
  certificates:
    - certFile: "$crtPath"
      keyFile: "$keyPath"
"@
    [IO.File]::WriteAllText((Join-Path $stackDir "dynamic.yml"), $dynamicYaml, [Text.Encoding]::UTF8)

    if (Get-Service "networker-traefik" -ErrorAction SilentlyContinue) {
        Start-Process -FilePath "sc.exe" -WindowStyle Hidden -Wait -ArgumentList @("stop","networker-traefik")
        Start-Process -FilePath "sc.exe" -WindowStyle Hidden -Wait -ArgumentList @("delete","networker-traefik")
    }
    $nssm = Invoke-EnsureNssm
    Start-Process -FilePath $nssm -WindowStyle Hidden -Wait -ArgumentList @("install","networker-traefik",$traefikExe,"--configfile",(Join-Path $stackDir "traefik.yml"))
    Start-Process -FilePath $nssm -WindowStyle Hidden -Wait -ArgumentList @("set","networker-traefik","AppDirectory",$stackDir)
    Start-Process -FilePath $nssm -WindowStyle Hidden -Wait -ArgumentList @("set","networker-traefik","Start","SERVICE_AUTO_START")
    Start-Process -FilePath "sc.exe" -WindowStyle Hidden -Wait -ArgumentList @("start","networker-traefik")

    Invoke-EnsureFirewallRule "Networker-Traefik-HTTP"  "TCP" @(8092)
    Invoke-EnsureFirewallRule "Networker-Traefik-HTTPS" "TCP" @(8455)
    Invoke-EnsureFirewallRule "Networker-Traefik-QUIC"  "UDP" @(8455)

    # Port-serving check — nssm reports "started" for a process that exits
    # immediately, so the service being registered proves nothing. The v0.28.150
    # windows-exec job caught exactly this: "OK Traefik serving" printed while
    # BOTH ports refused connections (same class as the caddy fixes).
    Start-Sleep -Seconds 3
    try {
        $null = Invoke-WebRequest -Uri "http://localhost:8092/" -UseBasicParsing -TimeoutSec 8
        Write-Ok "Traefik serving test page on ports 8092 (HTTP) / 8455 (HTTPS+H3)"
    } catch {
        Write-Err "Traefik service registered but port 8092 is not serving."
        Write-Err "  nssm status: $(& $nssm status networker-traefik 2>&1)"
        throw "traefik install failed: port not serving after start"
    }
}

# ── HAProxy (ports 8093 / 8456) ──────────────────────────────────────────────
function Invoke-SetupHAProxy {
    Invoke-NextStep "Set up HAProxy for HTTP stack comparison (ports 8093/8456)"
    Invoke-EnsureStaticSite

    $stackDir = Join-Path $script:NetworkerStackDir "haproxy"
    New-Item -ItemType Directory -Force $stackDir | Out-Null

    $haproxyCmd = Get-Command haproxy -ErrorAction SilentlyContinue
    $haproxyExe = if ($haproxyCmd) { $haproxyCmd.Source } else { Join-Path $stackDir "haproxy.exe" }
    if (-not (Test-Path $haproxyExe)) {
        # No official Windows binary. Community builds exist at
        # haproxy.debian.net / chocolatey ("haproxy"). Try choco first.
        $choco = Get-Command choco -ErrorAction SilentlyContinue
        if ($choco) {
            Write-Info "Installing HAProxy via Chocolatey..."
            Start-Process -FilePath $choco.Source -WindowStyle Hidden -Wait `
                -ArgumentList @("install","haproxy","-y","--no-progress")
            $machinePath = [System.Environment]::GetEnvironmentVariable("PATH", "Machine")
            $env:PATH    = "$machinePath;$env:PATH"
            $haproxyCmd = Get-Command haproxy -ErrorAction SilentlyContinue
            if ($haproxyCmd) { $haproxyExe = $haproxyCmd.Source }
        }
    }
    if (-not (Test-Path $haproxyExe)) {
        Write-Warn "HAProxy has no official Windows binary. Install manually from"
        Write-Warn "https://haproxy.debian.net/ or `choco install haproxy`, then re-run."
        Write-Warn "TODO: HAProxy Windows support is best-effort -- skipping service registration."
        return
    }

    $crtKey = Invoke-EnsureSelfSignedCert $stackDir "Networker HAProxy Test"
    # HAProxy needs a combined cert+key PEM file
    $combined = Join-Path $stackDir "networker.pem"
    $crtText = if (Test-Path $crtKey[0]) { Get-Content $crtKey[0] -Raw } else { "" }
    $keyText = if (Test-Path $crtKey[1]) { Get-Content $crtKey[1] -Raw } else { "" }
    [IO.File]::WriteAllText($combined, "$keyText`n$crtText", [Text.Encoding]::ASCII)

    # HAProxy is a pure proxy (no static file server). Forward everything to
    # networker-endpoint, matching the Linux haproxy convention.
    $combinedPath = $combined -replace '\\','/'
    $cfg = @"
global
    maxconn 2048
defaults
    mode http
    timeout connect 5s
    timeout client  30s
    timeout server  30s

frontend http-in
    bind *:8093
    default_backend endpoint-http

frontend https-in
    bind *:8456 ssl crt $combinedPath alpn h2,http/1.1
    default_backend endpoint-https

backend endpoint-http
    server ep 127.0.0.1:8080

backend endpoint-https
    server ep 127.0.0.1:8443 ssl verify none
"@
    $cfgPath = Join-Path $stackDir "haproxy.cfg"
    [IO.File]::WriteAllText($cfgPath, $cfg, [Text.Encoding]::ASCII)

    if (Get-Service "networker-haproxy" -ErrorAction SilentlyContinue) {
        Start-Process -FilePath "sc.exe" -WindowStyle Hidden -Wait -ArgumentList @("stop","networker-haproxy")
        Start-Process -FilePath "sc.exe" -WindowStyle Hidden -Wait -ArgumentList @("delete","networker-haproxy")
    }
    $nssm = Invoke-EnsureNssm
    Start-Process -FilePath $nssm -WindowStyle Hidden -Wait -ArgumentList @("install","networker-haproxy",$haproxyExe,"-f",$cfgPath)
    Start-Process -FilePath $nssm -WindowStyle Hidden -Wait -ArgumentList @("set","networker-haproxy","AppDirectory",$stackDir)
    Start-Process -FilePath $nssm -WindowStyle Hidden -Wait -ArgumentList @("set","networker-haproxy","Start","SERVICE_AUTO_START")
    Start-Process -FilePath "sc.exe" -WindowStyle Hidden -Wait -ArgumentList @("start","networker-haproxy")

    Invoke-EnsureFirewallRule "Networker-HAProxy-HTTP"  "TCP" @(8093)
    Invoke-EnsureFirewallRule "Networker-HAProxy-HTTPS" "TCP" @(8456)
    Write-Ok "HAProxy serving test page on ports 8093 (HTTP) / 8456 (HTTPS). HTTP/3 not supported on Windows builds."
}

# ── Apache httpd (ports 8094 / 8457) ─────────────────────────────────────────
function Invoke-SetupApache {
    Invoke-NextStep "Set up Apache httpd for HTTP stack comparison (ports 8094/8457)"
    Invoke-EnsureStaticSite

    $stackDir = Join-Path $script:NetworkerStackDir "apache"
    New-Item -ItemType Directory -Force $stackDir | Out-Null
    $httpdExe = Join-Path $stackDir "Apache24\bin\httpd.exe"

    if (-not (Test-Path $httpdExe)) {
        # Apache Lounge publishes canonical Windows binaries. Their download
        # URLs are not stable across versions; we use the versioned path and
        # fall back to a TODO warning when the download fails.
        Write-Info "Downloading Apache httpd from Apache Lounge..."
        $zipPath = Join-Path $env:TEMP "apache.zip"
        # Apache Lounge's URLs are version-pinned and rotate with each release
        # (the pinned 2.4.62 build 404'd by 2026-08 and every Windows Apache
        # cell died on it). Try a candidate list, newest first, with a browser
        # UA (their CDN rejects bare clients).
        $urls = @(
            "https://www.apachelounge.com/download/VS17/binaries/httpd-2.4.65-250724-win64-VS17.zip",
            "https://www.apachelounge.com/download/VS17/binaries/httpd-2.4.63-250207-win64-VS17.zip",
            "https://www.apachelounge.com/download/VS17/binaries/httpd-2.4.62-240904-win64-VS17.zip"
        )
        $got = $false
        foreach ($url in $urls) {
            try {
                Invoke-WebRequest -Uri $url -OutFile $zipPath -UseBasicParsing -TimeoutSec 60 `
                    -UserAgent "Mozilla/5.0 (Windows NT 10.0; Win64; x64) networker-installer"
                Expand-Archive -Path $zipPath -DestinationPath $stackDir -Force
                $got = $true
                break
            } catch {
                Write-Info "  candidate failed: $url"
            }
        }
        if (-not $got) {
            Write-Err "Apache download failed from all Apache Lounge candidates."
            Write-Err "Manually download from https://www.apachelounge.com/ and extract to $stackDir\Apache24."
            throw "apache install failed: no downloadable Windows binary"
        }
    }

    $crtKey = Invoke-EnsureSelfSignedCert $stackDir "Networker Apache Test"
    $siteRoot = $script:NetworkerSiteRoot -replace '\\','/'
    $serverRoot = "$stackDir\Apache24" -replace '\\','/'
    $crtPath = $crtKey[0] -replace '\\','/'
    $keyPath = $crtKey[1] -replace '\\','/'

    $cfg = @"
ServerRoot "$serverRoot"
Listen 8094
Listen 8457

LoadModule authz_core_module modules/mod_authz_core.so
LoadModule mime_module modules/mod_mime.so
LoadModule dir_module modules/mod_dir.so
LoadModule log_config_module modules/mod_log_config.so
LoadModule unixd_module modules/mod_unixd.so
LoadModule ssl_module modules/mod_ssl.so
LoadModule proxy_module modules/mod_proxy.so
LoadModule proxy_http_module modules/mod_proxy_http.so
LoadModule http2_module modules/mod_http2.so
LoadModule socache_shmcb_module modules/mod_socache_shmcb.so

ServerName localhost
DocumentRoot "$siteRoot"
<Directory "$siteRoot">
    Require all granted
    DirectoryIndex index.html
</Directory>

TypesConfig conf/mime.types
AddType application/octet-stream .bin

ErrorLog "logs/error.log"
LogLevel warn

<VirtualHost *:8094>
    DocumentRoot "$siteRoot"
    ProxyPass /page http://127.0.0.1:8080/page
    ProxyPassReverse /page http://127.0.0.1:8080/page
    ProxyPass /asset http://127.0.0.1:8080/asset
    ProxyPassReverse /asset http://127.0.0.1:8080/asset
</VirtualHost>

<VirtualHost *:8457>
    DocumentRoot "$siteRoot"
    Protocols h2 http/1.1
    SSLEngine on
    SSLCertificateFile "$crtPath"
    SSLCertificateKeyFile "$keyPath"
    SSLProxyEngine on
    SSLProxyVerify none
    SSLProxyCheckPeerCN off
    SSLProxyCheckPeerName off
    ProxyPass /page https://127.0.0.1:8443/page
    ProxyPassReverse /page https://127.0.0.1:8443/page
    ProxyPass /asset https://127.0.0.1:8443/asset
    ProxyPassReverse /asset https://127.0.0.1:8443/asset
</VirtualHost>
"@
    $cfgPath = Join-Path $stackDir "httpd.conf"
    [IO.File]::WriteAllText($cfgPath, $cfg, [Text.Encoding]::ASCII)

    if (Get-Service "networker-apache" -ErrorAction SilentlyContinue) {
        Start-Process -FilePath "sc.exe" -WindowStyle Hidden -Wait -ArgumentList @("stop","networker-apache")
        Start-Process -FilePath "sc.exe" -WindowStyle Hidden -Wait -ArgumentList @("delete","networker-apache")
    }
    # Apache has native service install via -k install
    Start-Process -FilePath $httpdExe -WindowStyle Hidden -Wait `
        -ArgumentList @("-k","install","-n","networker-apache","-f",$cfgPath)
    Start-Process -FilePath "sc.exe" -WindowStyle Hidden -Wait -ArgumentList @("start","networker-apache")

    Invoke-EnsureFirewallRule "Networker-Apache-HTTP"  "TCP" @(8094)
    Invoke-EnsureFirewallRule "Networker-Apache-HTTPS" "TCP" @(8457)
    Write-Ok "Apache serving test page on ports 8094 (HTTP) / 8457 (HTTPS). HTTP/3 not available in Apache httpd."
}

# ── IIS (ports 8082 / 8445, HTTP/3 via http.sys) ─────────────────────────────
# The PowerShell twin of install.sh `_iis_setup_powershell` (the payload the
# remote Windows deploy pushes through `az vm run-command`), so a LOCAL
# `install.ps1 -Setup iis` yields the same site the cloud endpoint VMs get:
# IIS + URL Rewrite + ARR, EnableHttp3/EnableHttp2* in http.sys (reboot needed
# the first time — the caller sees "REBOOT_NEEDED"), the generated static test
# site at C:\networker-static with the reverse-proxy web.config for
# /page /asset /download /upload /info /api /health /ws → networker-endpoint :8080,
# a self-signed cert, HTTPS 8445 (IP binding + optional -Fqdn SNI binding),
# the alt-svc h3 header and the firewall rules (TCP 8082/8445, UDP 8445).
# Until v0.28.208 this function was a placeholder that only bound :8082 —
# lab/ (Windows target) runs the real thing and validates :8445 incl. HTTP/3.
function Invoke-SetupIIS {
    Invoke-NextStep "Set up IIS for HTTP stack comparison (ports 8082/8445)"
    Invoke-EnsureTls12
    $siteRoot = "C:\networker-static"
    $fqdn = if ($Fqdn) { $Fqdn.Trim() } else { "" }

    # 1. IIS + URL Rewrite + ARR (reverse proxy for the dynamic endpoint routes)
    Write-Info "Installing IIS (Web-Server + WebSocket protocol)..."
    Import-Module ServerManager -ErrorAction SilentlyContinue
    # Web-WebSockets: ARR only tunnels WebSocket upgrades (the endpoint's /ws
    # echo used by the websocket probe) when the IIS WebSocket Protocol
    # feature is installed.
    Install-WindowsFeature -Name Web-Server,Web-WebSockets -IncludeManagementTools | Out-Null

    Write-Info "Installing URL Rewrite Module..."
    $urlRewriteMsi = Join-Path $env:TEMP "urlrewrite.msi"
    Invoke-WebRequest -Uri "https://download.microsoft.com/download/1/2/8/128E2E22-C1B9-44A4-BE2A-5859ED1D4592/rewrite_amd64_en-US.msi" `
        -OutFile $urlRewriteMsi -UseBasicParsing
    Start-Process msiexec.exe -ArgumentList "/i `"$urlRewriteMsi`" /quiet /norestart" -Wait -WindowStyle Hidden

    Write-Info "Installing ARR (Application Request Routing)..."
    $arrMsi = Join-Path $env:TEMP "arr.msi"
    Invoke-WebRequest -Uri "https://download.microsoft.com/download/E/9/8/E9849D6A-020E-47E4-9FD0-A023E99B54EB/requestRouter_amd64.msi" `
        -OutFile $arrMsi -UseBasicParsing
    Start-Process msiexec.exe -ArgumentList "/i `"$arrMsi`" /quiet /norestart" -Wait -WindowStyle Hidden

    Import-Module WebAdministration
    Set-WebConfigurationProperty -pspath "MACHINE/WEBROOT/APPHOST" `
        -filter "system.webServer/proxy" -name "enabled" -value "True"

    # 2. HTTP/3 + HTTP/2 in http.sys (Windows Server 2022+; needs a reboot once)
    Write-Info "Enabling HTTP/3 in http.sys (registry)..."
    $httpParams = "HKLM:\SYSTEM\CurrentControlSet\Services\HTTP\Parameters"
    if (-not (Test-Path $httpParams)) { New-Item -Path $httpParams -Force | Out-Null }
    $needsReboot = $false
    foreach ($prop in @("EnableHttp3","EnableHttp2Tls","EnableHttp2Cleartext")) {
        $cur = (Get-ItemProperty -Path $httpParams -Name $prop -ErrorAction SilentlyContinue).$prop
        if ($cur -ne 1) {
            Set-ItemProperty -Path $httpParams -Name $prop -Value 1 -Type DWord
            $needsReboot = $true
        }
    }

    # 3. Static test site (generated by networker-endpoint when present)
    New-Item -ItemType Directory -Path $siteRoot -Force | Out-Null
    $epExe = ""
    foreach ($cand in @("C:\networker\networker-endpoint.exe", (Join-Path $CargoBin "networker-endpoint.exe"))) {
        if (Test-Path $cand) { $epExe = $cand; break }
    }
    if (-not $epExe) {
        $epCmd = Get-Command networker-endpoint -ErrorAction SilentlyContinue
        if ($epCmd) { $epExe = $epCmd.Source }
    }
    $genSite = $false
    if ($epExe) {
        $prevErr = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $genSiteHelp = (& $epExe --help 2>&1) -join "`n"
        if ($genSiteHelp -match 'generate-site') {
            Write-Info "Generating static test site via networker-endpoint..."
            & $epExe generate-site $siteRoot --preset mixed --stack iis 2>&1 | Out-Null
            $genSite = Test-Path (Join-Path $siteRoot "index.html")
        }
        $ErrorActionPreference = $prevErr
    }
    if (-not $genSite) {
        Write-Info "Creating static test page (50 assets, mixed sizes)..."
        $html = "<!DOCTYPE html>`n<html><head><title>Networker Page Load Test</title>`n"
        $html += "<link rel=`"stylesheet`" href=`"style.css`">`n<link rel=`"icon`" href=`"data:,`">`n</head><body>`n"
        for ($i = 0; $i -lt 50; $i++) { $html += "<img src=`"asset-$i.bin`" width=`"1`" height=`"1`" alt=`"`">`n" }
        $html += "</body></html>"
        [IO.File]::WriteAllText("$siteRoot\index.html", $html, [Text.Encoding]::UTF8)
        [IO.File]::WriteAllText("$siteRoot\style.css", "body{margin:0}", [Text.Encoding]::UTF8)
        [IO.File]::WriteAllText("$siteRoot\health", '{"status":"ok","stack":"iis"}', [Text.Encoding]::UTF8)
        $sizes = @(512,512,512,512,512,2048,2048,2048,2048,2048,
                   4096,4096,4096,4096,4096,8192,8192,8192,8192,8192,
                   16384,16384,16384,16384,16384,32768,32768,32768,32768,32768,
                   65536,65536,65536,65536,65536,102400,102400,102400,102400,102400,
                   204800,204800,204800,204800,204800,409600,409600,614400,614400,1048576)
        $rng = New-Object Random
        for ($i = 0; $i -lt $sizes.Count; $i++) {
            $bytes = New-Object byte[] $sizes[$i]
            $rng.NextBytes($bytes)
            [IO.File]::WriteAllBytes("$siteRoot\asset-$i.bin", $bytes)
        }
    }

    # 3b. web.config: default doc, MIME types, reverse-proxy rules to the endpoint
    $webConfig = @"
<?xml version="1.0" encoding="UTF-8"?>
<configuration>
  <system.webServer>
    <defaultDocument>
      <files>
        <clear />
        <add value="index.html" />
      </files>
    </defaultDocument>
    <staticContent>
      <remove fileExtension="." />
      <mimeMap fileExtension="." mimeType="application/json" />
      <remove fileExtension=".bin" />
      <mimeMap fileExtension=".bin" mimeType="application/octet-stream" />
    </staticContent>
    <rewrite>
      <rules>
        <rule name="Proxy /page to endpoint" stopProcessing="true">
          <match url="^page(.*)" />
          <action type="Rewrite" url="http://127.0.0.1:8080/page{R:1}" />
        </rule>
        <rule name="Proxy /asset to endpoint" stopProcessing="true">
          <match url="^asset$" />
          <conditions>
            <add input="{QUERY_STRING}" pattern=".+" />
          </conditions>
          <action type="Rewrite" url="http://127.0.0.1:8080/asset?{C:0}" appendQueryString="false" />
        </rule>
        <rule name="Proxy throughput + info + apibench + ws to endpoint" stopProcessing="true">
          <match url="^(download|upload|info|api|health|ws)(.*)" />
          <action type="Rewrite" url="http://127.0.0.1:8080/{R:1}{R:2}" />
        </rule>
      </rules>
    </rewrite>
  </system.webServer>
</configuration>
"@
    [IO.File]::WriteAllText("$siteRoot\web.config", $webConfig, (New-Object System.Text.UTF8Encoding $false))

    # 4. Self-signed certificate (SAN: localhost, hostname[, fqdn])
    $dnsNames = @("localhost", $env:COMPUTERNAME)
    if ($fqdn) { $dnsNames += $fqdn }
    $cert = New-SelfSignedCertificate -DnsName $dnsNames -CertStoreLocation "Cert:\LocalMachine\My" `
        -NotAfter (Get-Date).AddYears(1) -FriendlyName "Networker IIS Test"
    $thumbprint = $cert.Thumbprint

    # 5. Site + bindings
    if (Get-Website -Name "Default Web Site" -ErrorAction SilentlyContinue) { Remove-Website -Name "Default Web Site" }
    if (Get-Website -Name "networker-iis"    -ErrorAction SilentlyContinue) { Remove-Website -Name "networker-iis" }
    New-Website -Name "networker-iis" -PhysicalPath $siteRoot -Port 8082 -Force | Out-Null
    if ($fqdn) {
        New-WebBinding -Name "networker-iis" -Protocol "https" -Port 8445 -HostHeader $fqdn -SslFlags 1
        $sniBinding = Get-WebBinding -Name "networker-iis" -Protocol "https" -Port 8445 | Where-Object { $_.sslFlags -eq 1 }
        $sniBinding.AddSslCertificate($thumbprint, "My")
        Write-Info "HTTPS binding: hostname=$fqdn (SNI)"
    }
    New-WebBinding -Name "networker-iis" -Protocol "https" -Port 8445 -SslFlags 0
    $ipBinding = Get-WebBinding -Name "networker-iis" -Protocol "https" -Port 8445 | Where-Object { $_.sslFlags -eq 0 }
    $ipBinding.AddSslCertificate($thumbprint, "My")
    Set-WebConfigurationProperty -pspath "IIS:\Sites\networker-iis" `
        -filter "system.webServer/httpProtocol/customHeaders" -name "." `
        -value @{name="alt-svc"; value="h3="":8445""; ma=86400"}
    Start-Website -Name "networker-iis"

    # 6. Firewall
    Invoke-EnsureFirewallRule "Networker-IIS-HTTP"  "TCP" @(8082)
    Invoke-EnsureFirewallRule "Networker-IIS-HTTPS" "TCP" @(8445)
    Invoke-EnsureFirewallRule "Networker-IIS-QUIC"  "UDP" @(8445)

    Write-Ok "IIS serving test page on ports 8082 (HTTP) / 8445 (HTTPS + HTTP/3), proxying dynamic routes to networker-endpoint :8080."
    if ($needsReboot) {
        Write-Warn "HTTP/3 registry keys were just set -- http.sys needs a REBOOT before :8445 answers QUIC."
        Write-Host "REBOOT_NEEDED"
    } else {
        Write-Info "HTTP/3 registry already set -- no reboot needed"
    }
}

# ── Dispatcher: map http_stacks names to setup functions ──────────────────────
function Invoke-HttpStackSetup ($stacks) {
    # $stacks: array or comma-separated string of stack names.
    if (-not $stacks) { return }
    if ($stacks -is [string]) { $stacks = $stacks -split ',' }
    foreach ($raw in $stacks) {
        $s = $raw.Trim().ToLower()
        if ([string]::IsNullOrEmpty($s)) { continue }
        switch ($s) {
            "nginx"   { Write-Warn "nginx setup on Windows is not supported -- use install.sh on Linux or pick IIS." }
            "iis"     { Invoke-SetupIIS }
            "caddy"   { Invoke-SetupCaddy }
            "traefik" { Invoke-SetupTraefik }
            "haproxy" { Invoke-SetupHAProxy }
            "apache"  { Invoke-SetupApache }
            default   { Write-Warn "Unknown http_stack '$s' (valid: nginx, iis, caddy, apache, haproxy, traefik)" }
        }
    }
}

# ── Reference-API language servers (Windows) ─────────────────────────────────
# The PowerShell mirror of install.sh's deploy_benchmark_server: install the
# language runtime (chocolatey when missing), build the committed reference
# API, run it on $port in application mode (plain HTTP, BENCH_USE_TLS=0), and
# persist it across reboots via a schtasks ONSTART wrapper. Finally retarget
# the endpoint's /api at the language server (--api-upstream, endpoint
# >= 0.28.203) so apibench measures the LANGUAGE behind this target.
# Windows-viable set only: cpp (MSVC+boost build), ruby (devkit gem builds)
# and php (swoole is Linux-only) are deliberately excluded; AOT variants need
# the VS C++ toolchain. Everything here is idempotent.

$script:WindowsBenchLangs = @(
    "csharp-net48", "csharp-net8", "csharp-net9", "csharp-net10",
    "go", "nodejs", "python", "java"
)

function Get-BenchRepo {
    # Shallow clone (or reuse) the repo for the committed reference APIs +
    # shared dataset. Returns the reference-apis directory path.
    $benchRoot = "C:\networker-bench"
    $repoDir   = Join-Path $benchRoot "repo"
    New-Item -ItemType Directory -Force $benchRoot | Out-Null
    # CI hook: reuse an existing checkout instead of downloading (the installer
    # execution job runs -BenchmarkServer against the PR's own tree).
    if ($env:NETWORKER_BENCH_REPO_DIR -and (Test-Path (Join-Path $env:NETWORKER_BENCH_REPO_DIR "benchmarks\reference-apis"))) {
        return (Join-Path $env:NETWORKER_BENCH_REPO_DIR "benchmarks\reference-apis")
    }
    # Zip download, not git clone: fresh Windows Server VMs ship neither git
    # nor chocolatey (field-caught 2026-08-14 on the first real net48 deploy;
    # CI runners have both preinstalled and could not see it).
    Write-Info "Fetching reference APIs (repo zip)..."
    $zip = Join-Path $benchRoot "repo.zip"
    Invoke-WebRequest -Uri "$RepoHttps/archive/refs/heads/main.zip" -OutFile $zip -UseBasicParsing
    if (Test-Path $repoDir) { Remove-Item $repoDir -Recurse -Force }
    Expand-Archive -Path $zip -DestinationPath $benchRoot -Force
    Remove-Item $zip -Force
    $extracted = Get-ChildItem $benchRoot -Directory -Filter "networker-tester-*" | Select-Object -First 1
    if (-not $extracted) { throw "repo zip extraction produced no networker-tester-* directory" }
    Move-Item $extracted.FullName $repoDir -Force
    return (Join-Path $repoDir "benchmarks\reference-apis")
}

function Install-Chocolatey {
    # Fresh Windows Server VMs do NOT ship chocolatey (GH runners do, which
    # is why CI never hit this). Official bootstrap, idempotent.
    if (Get-Command choco -ErrorAction SilentlyContinue) { return }
    Write-Info "Bootstrapping chocolatey..."
    Set-ExecutionPolicy Bypass -Scope Process -Force
    [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.ServicePointManager]::SecurityProtocol -bor 3072
    # Download-to-file then run (PSScriptAnalyzer bans Invoke-Expression).
    $chocoInstaller = Join-Path $env:TEMP "choco-install.ps1"
    Invoke-WebRequest -Uri 'https://community.chocolatey.org/install.ps1' -OutFile $chocoInstaller -UseBasicParsing
    & $chocoInstaller
    $env:PATH = "$env:PATH;$env:ProgramData\chocolatey\bin"
    if (-not (Get-Command choco -ErrorAction SilentlyContinue)) { throw "chocolatey bootstrap failed" }
}

function Install-BenchRuntime ($lang) {
    # Ensure the language runtime exists; chocolatey fills gaps. Returns the
    # tool command name to sanity-check afterwards.
    switch -Wildcard ($lang) {
        "csharp-net48" {
            # csc.exe ships with .NET Framework 4.8 on Windows Server — no install.
            $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
            if (-not (Test-Path $csc)) { throw ".NET Framework 4.8 csc.exe not found at $csc" }
            return $csc
        }
        "csharp-*" {
            $channel = switch ($lang) {
                "csharp-net8"  { "8.0" }
                "csharp-net9"  { "9.0" }
                "csharp-net10" { "10.0" }
            }
            $sdks = & dotnet --list-sdks 2>$null
            if (-not ($sdks -match "^$([regex]::Escape($channel))")) {
                Write-Info "Installing .NET SDK $channel..."
                $di = Join-Path $env:TEMP "dotnet-install.ps1"
                Invoke-WebRequest -Uri "https://dot.net/v1/dotnet-install.ps1" -OutFile $di -UseBasicParsing
                & $di -Channel $channel -InstallDir "$env:ProgramFiles\dotnet" | Out-Null
            }
            return "dotnet"
        }
        "go" {
            if (-not (Get-Command go -ErrorAction SilentlyContinue)) {
                Write-Info "Installing Go via chocolatey..."
                Install-Chocolatey
                choco install golang -y --no-progress | Out-Null
                $env:PATH = "$env:PATH;C:\Program Files\Go\bin"
            }
            return "go"
        }
        "nodejs" {
            if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
                Write-Info "Installing Node.js via chocolatey..."
                Install-Chocolatey
                choco install nodejs-lts -y --no-progress | Out-Null
                $env:PATH = "$env:PATH;C:\Program Files\nodejs"
            }
            return "node"
        }
        "python" {
            if (-not (Get-Command python -ErrorAction SilentlyContinue)) {
                Write-Info "Installing Python via chocolatey..."
                Install-Chocolatey
                choco install python -y --no-progress | Out-Null
                $env:PATH = "$env:PATH;C:\Python312;C:\Python312\Scripts"
            }
            return "python"
        }
        "java" {
            if (-not (Get-Command javac -ErrorAction SilentlyContinue)) {
                Write-Info "Installing Temurin JDK via chocolatey..."
                Install-Chocolatey
                choco install temurin -y --no-progress | Out-Null
                $jdk = Get-ChildItem "C:\Program Files\Eclipse Adoptium" -Directory -ErrorAction SilentlyContinue | Select-Object -First 1
                if ($jdk) { $env:PATH = "$env:PATH;$($jdk.FullName)\bin" }
            }
            return "javac"
        }
    }
}

function Invoke-BenchmarkServerSetup ($lang, $port) {
    if ($lang -notin $script:WindowsBenchLangs) {
        Write-Err ("'$lang' is not deployable on Windows. Supported: " +
            ($script:WindowsBenchLangs -join ", ") +
            ". (cpp/ruby/php and AOT variants are Linux-only in this path.)")
        exit 1
    }

    $apiDir   = Get-BenchRepo
    $benchDir = "C:\networker-bench"
    $runtime  = Install-BenchRuntime $lang

    # Shared dataset (API-SPEC §2 — load failure is FATAL in every server).
    $dataSrc = Join-Path $apiDir "shared\bench-data.json"
    $data    = Join-Path $benchDir "bench-data.json"
    if (Test-Path $dataSrc) { Copy-Item $dataSrc $data -Force }

    # ── Build + compose the start command ────────────────────────────────
    Write-Info "Building $lang reference API..."
    $startCmd = $null
    switch -Wildcard ($lang) {
        "csharp-net48" {
            $exe = Join-Path $benchDir "csharp-net48.exe"
            & $runtime /nologo /out:$exe `
                /reference:System.IO.Compression.dll `
                /reference:System.Web.Extensions.dll `
                /target:exe (Join-Path $apiDir "csharp-net48\Server.cs")
            if ($LASTEXITCODE -ne 0) { Write-Err "csc.exe compilation failed"; exit 1 }
            $startCmd = "`"$exe`""
            break
        }
        "csharp-*" {
            $srcDir = Join-Path $apiDir $lang
            if (-not (Test-Path $srcDir)) { $srcDir = Join-Path $apiDir "csharp" }
            $outDir = Join-Path $benchDir $lang
            Push-Location $srcDir
            & dotnet publish -c Release -o $outDir --nologo -v quiet
            $ok = ($LASTEXITCODE -eq 0)
            Pop-Location
            if (-not $ok) { Write-Err "dotnet publish failed for $lang"; exit 1 }
            $exe = Get-ChildItem $outDir -Filter "*.exe" | Select-Object -First 1
            if (-not $exe) { Write-Err "no exe produced for $lang"; exit 1 }
            $startCmd = "`"$($exe.FullName)`""
            break
        }
        "go" {
            $exe = Join-Path $benchDir "go-server.exe"
            Push-Location (Join-Path $apiDir "go")
            & go build -o $exe .
            $ok = ($LASTEXITCODE -eq 0)
            Pop-Location
            if (-not $ok) { Write-Err "go build failed"; exit 1 }
            $startCmd = "`"$exe`""
            break
        }
        "nodejs" {
            Push-Location (Join-Path $apiDir "nodejs")
            & npm install --quiet --no-audit --no-fund | Out-Null
            Pop-Location
            $startCmd = "`"$((Get-Command node).Source)`" `"$(Join-Path $apiDir 'nodejs\server.js')`""
            break
        }
        "python" {
            $venv = Join-Path $benchDir "pyenv"
            if (-not (Test-Path $venv)) { & python -m venv $venv }
            & (Join-Path $venv "Scripts\pip.exe") install --quiet -r (Join-Path $apiDir "python\requirements.txt")
            if ($LASTEXITCODE -ne 0) { Write-Err "pip install failed"; exit 1 }
            $startCmd = "`"$(Join-Path $venv 'Scripts\hypercorn.exe')`" server:app --bind 0.0.0.0:$port"
            break
        }
        "java" {
            $buildDir = Join-Path $benchDir "java-build"
            New-Item -ItemType Directory -Force $buildDir | Out-Null
            & javac -d $buildDir (Join-Path $apiDir "java\*.java")
            if ($LASTEXITCODE -ne 0) { Write-Err "javac failed"; exit 1 }
            $startCmd = "`"$((Get-Command java).Source)`" -cp `"$buildDir`" Server"
            break
        }
    }

    # ── Wrapper .cmd: env vars don't ride schtasks /TR ────────────────────
    # Application mode: plain HTTP on $port (no certs provisioned => the
    # servers fall back to http per API-SPEC audit F8).
    $wrapper = Join-Path $benchDir "run-$lang.cmd"
    $workDir = if ($lang -eq "python") { Join-Path $apiDir "python" } else { $benchDir }
    @(
        "@echo off",
        "set BENCH_PORT=$port",
        "set BENCH_USE_TLS=0",
        "set BENCH_DATA_PATH=$data",
        "cd /d `"$workDir`"",
        "$startCmd >> `"$benchDir\$lang.log`" 2>&1"
    ) | Set-Content -Path $wrapper -Encoding ascii

    # Kill any previous language server holding the port, start, persist.
    $owner = (Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1).OwningProcess
    if ($owner) {
        Write-Info "Stopping previous language server (pid $owner) on port $port..."
        Stop-Process -Id $owner -Force -ErrorAction SilentlyContinue
    }
    Write-Info "Starting $lang reference API on port $port..."
    Start-Process -FilePath "cmd.exe" -ArgumentList "/c", $wrapper -WindowStyle Hidden
    schtasks /Create /TN "NetworkerBench" /TR "cmd.exe /c `"$wrapper`"" /SC ONSTART /RU SYSTEM /F | Out-Null

    # Verify the language server itself (its /health names the runtime, §5.1).
    $healthy = $false
    foreach ($i in 1..30) {
        Start-Sleep -Seconds 2
        try {
            $r = Invoke-WebRequest -Uri "http://localhost:$port/health" -UseBasicParsing -TimeoutSec 3
            if ($r.StatusCode -eq 200) { $healthy = $true; break }
        } catch { Write-Verbose "waiting for ${lang}: $($_.Exception.Message)" }
    }
    if (-not $healthy) {
        Write-Err "$lang server not healthy on :$port after 60s - see $benchDir\$lang.log"
        Get-Content "$benchDir\$lang.log" -Tail 20 -ErrorAction SilentlyContinue | Write-Host
        exit 1
    }
    Write-Ok "$lang reference API healthy on :$port"

    # ── Retarget the endpoint /api at the language (--api-upstream) ───────
    $epExe = "C:\networker\networker-endpoint.exe"
    if (-not (Test-Path $epExe)) {
        $cmd = Get-Command networker-endpoint -ErrorAction SilentlyContinue
        if ($cmd) { $epExe = $cmd.Source }
    }
    if (-not (Test-Path $epExe)) {
        Write-Err "networker-endpoint.exe not found - cannot route /api to the language server"
        exit 1
    }
    Write-Info "Retargeting endpoint /api -> 127.0.0.1:$port (--api-upstream)..."
    Stop-Process -Name "networker-endpoint" -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 1
    schtasks /Create /TN "NetworkerEndpoint" /TR "`"$epExe`" --api-upstream 127.0.0.1:$port" /SC ONSTART /RU SYSTEM /F | Out-Null
    Start-Process -FilePath $epExe -ArgumentList "--api-upstream", "127.0.0.1:$port" -WindowStyle Hidden

    # End-to-end proof: the ENDPOINT's /health must self-report the upstream
    # (services.api_upstream, endpoint >= 0.28.203 — older binaries silently
    # ignore unknown flags is NOT a risk: clap rejects them, so the process
    # dies and this check goes red instead of lying).
    $wired = $false
    foreach ($i in 1..15) {
        Start-Sleep -Seconds 2
        try {
            $h = Invoke-WebRequest -Uri "http://localhost:8080/health" -UseBasicParsing -TimeoutSec 3
            $j = $h.Content | ConvertFrom-Json
            if ($j.services.api_upstream -eq "127.0.0.1:$port") { $wired = $true; break }
        } catch { Write-Verbose "waiting for endpoint restart: $($_.Exception.Message)" }
    }
    if (-not $wired) {
        Write-Err "endpoint did not come back reporting api_upstream - is it >= 0.28.203? (upgrade the target, then re-run)"
        exit 1
    }
    Write-Ok "endpoint /api now measures $lang (services.api_upstream=127.0.0.1:$port)"
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

# -Setup fast-path: invoked remotely by install.sh via az vm run-command to
# install a single reverse-proxy stack on an already-provisioned Windows VM.
# Bypasses component selection, downloads, and deploy flow.
if ($Setup) {
    $validStacks = @("iis","caddy","traefik","haproxy","apache")
    $wanted = $Setup.ToLower()
    if ($wanted -notin $validStacks) {
        Write-Err "Invalid -Setup value '$Setup'. Use: $($validStacks -join ', ')."
        exit 1
    }
    Invoke-HttpStackSetup $wanted
    exit 0
}

# -BenchmarkServer fast-path: same remote-invocation contract as -Setup.
if ($BenchmarkServer) {
    Invoke-BenchmarkServerSetup $BenchmarkServer.ToLower() $BenchmarkPort
    exit 0
}

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
