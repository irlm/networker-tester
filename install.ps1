#Requires -Version 5.1
# ──────────────────────────────────────────────────────────────────────────────
# Networker Tester – Windows interactive installer (rustup-style)
#
# Usage (piped):
#   irm <raw-gist-url>/install.ps1 | iex
#
# Usage (downloaded):
#   .\install.ps1 [-Component tester|endpoint|both] [-Yes] [-SkipSshCheck]
#                 [-SkipRust] [-Help]
#
# Prerequisites:
#   - Git for Windows (includes ssh.exe) – https://git-scm.com/
#   - SSH key configured for github.com in %USERPROFILE%\.ssh\
# ──────────────────────────────────────────────────────────────────────────────
param(
    [string]$Component    = "both",
    [switch]$Yes,
    [switch]$SkipSshCheck,
    [switch]$SkipRust,
    [switch]$Help
)

$ErrorActionPreference = "Stop"

$ScriptVersion = "0.12.11"
$RepoSsh       = "ssh://git@github.com/irlm/networker-tester"
$CargoBin      = Join-Path $env:USERPROFILE ".cargo\bin"

# ── Print helpers ──────────────────────────────────────────────────────────────
function Write-Ok   ($msg) { Write-Host "  v " -NoNewline -ForegroundColor Green;   Write-Host $msg }
function Write-Warn ($msg) { Write-Host "  ! " -NoNewline -ForegroundColor Yellow;  Write-Host $msg }
function Write-Err  ($msg) { Write-Host "  x $msg" -ForegroundColor Red }
function Write-Info ($msg) { Write-Host "  > " -NoNewline -ForegroundColor Cyan;    Write-Host $msg }
function Write-Dim  ($msg) { Write-Host "    $msg" -ForegroundColor DarkGray }

function Write-Banner {
    Write-Host ""
    Write-Host ("=" * 58) -ForegroundColor Cyan
    Write-Host ("      Networker Tester Installer  v" + $ScriptVersion) -ForegroundColor Cyan
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
    Write-Host "  -Yes           Non-interactive: accept all defaults"
    Write-Host "  -SkipSshCheck  Skip the GitHub SSH connectivity test"
    Write-Host "  -SkipRust      Skip Rust installation (assume cargo is available)"
    Write-Host "  -Help          Show this help message"
    Write-Host ""
    Write-Host "Examples:"
    Write-Host "  .\install.ps1 -Component tester"
    Write-Host "  .\install.ps1 -Yes -Component endpoint"
    Write-Host "  .\install.ps1 -SkipRust -Component both"
}

# ── Script-level state ($script: prefix required to mutate from inside functions)
$script:DoSshCheck        = $true
$script:DoRustInstall     = $false
$script:DoInstallTester   = $true
$script:DoInstallEndpoint = $true
$script:RustExists        = $false
$script:RustVer           = "not installed"
$script:SysOs             = ""
$script:SysArch           = ""
$script:StepNum           = 0

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

    if (-not $script:RustExists -and -not $SkipRust) {
        $script:DoRustInstall = $true
    }

    if ($SkipSshCheck) {
        $script:DoSshCheck = $false
    }

    switch ($Component) {
        "tester"   { $script:DoInstallEndpoint = $false }
        "endpoint" { $script:DoInstallTester   = $false }
    }
}

# ── Display helpers ────────────────────────────────────────────────────────────
function Show-SystemInfo {
    Write-Section "System Information"
    Write-Host ""
    Write-Host ("    {0,-22} {1}" -f "OS:",           $script:SysOs)
    Write-Host ("    {0,-22} {1}" -f "Architecture:", $script:SysArch)
    Write-Host ("    {0,-22} {1}" -f "User home:",    $env:USERPROFILE)
    Write-Host ("    {0,-22} {1}" -f "Rust / cargo:", $script:RustVer)
    Write-Host ("    {0,-22} {1}" -f "Install to:",   $CargoBin)
}

function Show-Plan {
    Write-Section "Installation Plan"
    Write-Host ""
    $step = 1

    if ($script:DoSshCheck) {
        Write-Host ("    {0}. SSH check              Verify GitHub SSH access" -f $step)
        $step++
    } else {
        Write-Host "    -. SSH check              (skipped)" -ForegroundColor DarkGray
    }

    if ($script:DoRustInstall) {
        Write-Host ("    {0}. Install Rust           Download rustup-init.exe from win.rustup.rs" -f $step)
        $step++
    } elseif (-not $script:RustExists) {
        Write-Host "    -. Install Rust            (skipped -- -SkipRust)" -ForegroundColor DarkGray
    } else {
        Write-Host ("    -. Install Rust            (skip -- already installed: {0})" -f $script:RustVer) -ForegroundColor DarkGray
    }

    if ($script:DoInstallTester) {
        Write-Host ("    {0}. Install networker-tester    cargo install from private Git repo" -f $step)
        $step++
    }
    if ($script:DoInstallEndpoint) {
        Write-Host ("    {0}. Install networker-endpoint  cargo install from private Git repo" -f $step)
        $step++
    }

    Write-Host ""
    Write-Dim "Repository:  $RepoSsh"
    Write-Dim "Source code is compiled locally -- no pre-built binaries are downloaded."
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
            "1" { return }
            "2" { Invoke-CustomizeFlow; return }
            "3" { Write-Host ""; Write-Host "Installation cancelled."; exit 0 }
            default { Write-Warn "Please enter 1, 2, or 3." }
        }
    }
}

# ── Yes/No helper ──────────────────────────────────────────────────────────────
function Invoke-AskYN ($prompt, $default) {
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

# ── Customize flow ─────────────────────────────────────────────────────────────
function Invoke-CustomizeFlow {
    Write-Section "Customize Installation"
    Write-Host ""

    # SSH check
    $script:DoSshCheck = Invoke-AskYN "Run SSH connectivity check for GitHub?" "y"

    # Rust install – only relevant when Rust is not already present
    if (-not $script:RustExists) {
        Write-Host ""
        $script:DoRustInstall = Invoke-AskYN "Install Rust via rustup (win.rustup.rs)?" "y"
        if (-not $script:DoRustInstall) {
            Write-Host ""
            Write-Warn "Rust is not installed -- cargo must be on PATH before proceeding."
            Write-Host "  Install manually: https://rustup.rs"
            Write-Host "  Then re-run this script with -SkipRust"
        }
    }

    # Component selection
    Write-Host ""
    Write-Host "  Which components do you want to install?"
    Write-Host ""
    Write-Host "    1) Both  (networker-tester + networker-endpoint)  [default]"
    Write-Host "    2) tester only   -- the diagnostic CLI client"
    Write-Host "    3) endpoint only -- the target test server"
    Write-Host ""

    $compAns = Read-Host "  Choice [1]"
    if ([string]::IsNullOrWhiteSpace($compAns)) { $compAns = "1" }
    switch ($compAns.Trim()) {
        "2" { $script:DoInstallTester = $true;  $script:DoInstallEndpoint = $false }
        "3" { $script:DoInstallTester = $false; $script:DoInstallEndpoint = $true  }
        default { $script:DoInstallTester = $true; $script:DoInstallEndpoint = $true }
    }

    # Show revised plan and confirm
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

# ── Step execution helpers ─────────────────────────────────────────────────────
function Invoke-NextStep ($title) {
    $script:StepNum++
    Write-StepHeader $script:StepNum $title
}

function Invoke-SshStep {
    Invoke-NextStep "Verify GitHub SSH access"

    $sshCmd = Get-Command ssh -ErrorAction SilentlyContinue
    $sshExe = if ($sshCmd) { $sshCmd.Source } else { $null }
    if (-not $sshExe) {
        Write-Host ""
        Write-Err "ssh.exe not found."
        Write-Host "  Install Git for Windows (https://git-scm.com/) which bundles OpenSSH." -ForegroundColor Red
        exit 1
    }

    Write-Info "Connecting to git@github.com..."

    # ssh -T always exits 1 on GitHub (by design). Lower $ErrorActionPreference so
    # PS 5.1 does not throw NativeCommandError on the non-zero exit code.
    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $sshOutput = & $sshExe -o BatchMode=yes -o StrictHostKeyChecking=accept-new `
                            -o ConnectTimeout=10 -T git@github.com 2>&1
    $ErrorActionPreference = $prevErr

    if ($sshOutput -match "successfully authenticated") {
        Write-Ok "SSH access confirmed"
    } else {
        Write-Host ""
        Write-Err "SSH authentication to GitHub failed."
        Write-Host "  Output: $sshOutput" -ForegroundColor Red
        Write-Host "  Ensure your SSH key is loaded and has access to the private repo."
        Write-Host "  Test manually: ssh -T git@github.com"
        exit 1
    }
}

function Invoke-RustInstallStep {
    Invoke-NextStep "Install Rust via rustup"

    # Choose the correct rustup-init URL for this architecture
    $arch       = $env:PROCESSOR_ARCHITECTURE
    $rustupUrl  = if ($arch -eq "ARM64") { "https://win.rustup.rs/aarch64" } else { "https://win.rustup.rs/x86_64" }
    $rustupExe  = Join-Path $env:TEMP "rustup-init.exe"

    Write-Info "Downloading rustup from $rustupUrl ..."
    Invoke-WebRequest -Uri $rustupUrl -OutFile $rustupExe -UseBasicParsing
    & $rustupExe -y --no-modify-path
    Remove-Item $rustupExe -Force -ErrorAction SilentlyContinue

    # Add cargo bin to PATH for this session
    if ($env:PATH -notlike "*$CargoBin*") {
        $env:PATH = "$CargoBin;$env:PATH"
    }

    $script:RustVer = (& rustc --version 2>&1)
    Write-Ok ("Rust installed: " + $script:RustVer)
}

function Invoke-EnsureCargoEnv {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        if ($env:PATH -notlike "*$CargoBin*") {
            $env:PATH = "$CargoBin;$env:PATH"
        }
    }
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Write-Err "cargo not found -- cannot install binaries."
        Write-Host "  If Rust was just installed, open a new terminal and re-run this script."
        exit 1
    }
}

function Invoke-CargoInstallStep ($binary) {
    Invoke-NextStep "Install $binary"
    Write-Info "Building and installing $binary from source..."
    Write-Dim "This compiles from the private Git repo and may take a few minutes."
    Write-Host ""

    # CARGO_NET_GIT_FETCH_WITH_CLI=true makes cargo delegate git operations to
    # the system git binary rather than libgit2, which reliably picks up the
    # SSH agent on Windows.
    # --force rebuilds unconditionally even when cargo's SHA cache is current.
    $env:CARGO_NET_GIT_FETCH_WITH_CLI = "true"
    & cargo install --git $RepoSsh $binary --locked --force
    $env:CARGO_NET_GIT_FETCH_WITH_CLI = $null

    $installedCmd  = Get-Command $binary -ErrorAction SilentlyContinue
    $installedPath = if ($installedCmd) { $installedCmd.Source } else { "$CargoBin\$binary.exe" }
    $installedVer  = if ($installedCmd) { (& $binary --version 2>&1) } else { "unknown" }
    Write-Host ""
    Write-Ok "$binary installed -> $installedPath  ($installedVer)"
}

# ── Completion summary ─────────────────────────────────────────────────────────
function Show-Completion {
    Write-Host ""
    Write-Host ("=" * 58) -ForegroundColor Green
    Write-Host "  Installation complete!" -ForegroundColor Green
    Write-Host ("=" * 58) -ForegroundColor Green
    Write-Host ""

    # PATH notice
    if ($env:PATH -notlike "*$CargoBin*") {
        Write-Warn "$CargoBin is not in PATH for this session."
        Write-Host ""
        Write-Host "  Run now (activates for this terminal session):"
        Write-Host ('    $env:PATH = "' + $CargoBin + ';$env:PATH"')
        Write-Host ""
        Write-Host "  Make permanent (User scope):"
        Write-Host ('    [Environment]::SetEnvironmentVariable("PATH", "' + $CargoBin + ';$env:PATH", "User")')
        Write-Host ""
    }

    if ($script:DoInstallTester) {
        Write-Host "  networker-tester quick start:" -ForegroundColor White
        Write-Host "    networker-tester --help"
        Write-Host "    networker-tester --target http://localhost:8080/health --modes http1 --runs 3"
        Write-Host ""
    }

    if ($script:DoInstallEndpoint) {
        Write-Host "  networker-endpoint quick start:" -ForegroundColor White
        Write-Host "    networker-endpoint"
        Write-Host "    # Listens on :8080 HTTP, :8443 HTTPS/H2/H3, :9998 UDP throughput, :9999 UDP echo"
        Write-Host ""
    }
}

# ── Entry point ────────────────────────────────────────────────────────────────
if ($Help) { Show-Help; exit 0 }

# Validate -Component (when piped through iex the ValidateSet is not enforced)
if ($Component -notin @("tester", "endpoint", "both")) {
    Write-Err "Invalid -Component value '$Component'. Use: tester, endpoint, or both."
    exit 1
}

Invoke-DiscoverSystem

Write-Banner
Show-SystemInfo
Show-Plan
Invoke-MainPrompt

if ($script:DoSshCheck)    { Invoke-SshStep }
if ($script:DoRustInstall) { Invoke-RustInstallStep }
Invoke-EnsureCargoEnv
if ($script:DoInstallTester)   { Invoke-CargoInstallStep "networker-tester" }
if ($script:DoInstallEndpoint) { Invoke-CargoInstallStep "networker-endpoint" }

Show-Completion
