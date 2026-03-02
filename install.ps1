#Requires -Version 5.1
# ──────────────────────────────────────────────────────────────────────────────
# Networker Tester – Windows installer (PowerShell)
#
# Installs either the diagnostic CLIENT or the test ENDPOINT from the private
# GitHub repo using SSH (your existing SSH key is used – no token required).
#
# Usage:
#   irm <raw-gist-url>/install.ps1 | iex  (defaults to tester)
#
#   Or download and run directly:
#   .\install.ps1 -Component tester
#   .\install.ps1 -Component endpoint
#
# Prerequisites:
#   - Git for Windows (includes ssh.exe) – https://git-scm.com/
#   - SSH key configured for github.com in %USERPROFILE%\.ssh\
# ──────────────────────────────────────────────────────────────────────────────
param(
    [Parameter(Mandatory = $false)]
    [ValidateSet("tester", "endpoint")]
    [string]$Component = "tester"
)

$ErrorActionPreference = "Stop"

$RepoSsh = "ssh://git@github.com/irlm/networker-tester"
$Binary  = if ($Component -eq "endpoint") { "networker-endpoint" } else { "networker-tester" }

function Write-Info    ($msg) { Write-Host "[info]  $msg" -ForegroundColor Cyan }
function Write-Success ($msg) { Write-Host "[ok]    $msg" -ForegroundColor Green }
function Write-Warn    ($msg) { Write-Host "[warn]  $msg" -ForegroundColor Yellow }

# ── Check SSH access to GitHub ────────────────────────────────────────────────
Write-Info "Checking SSH access to GitHub..."

$sshCmd = Get-Command ssh -ErrorAction SilentlyContinue
$sshExe = if ($sshCmd) { $sshCmd.Source } else { $null }
if (-not $sshExe) {
    Write-Host ""
    Write-Host "  ssh.exe not found." -ForegroundColor Red
    Write-Host "  Install Git for Windows (https://git-scm.com/) which bundles OpenSSH."
    exit 1
}

$sshOutput = & $sshExe -o BatchMode=yes -o StrictHostKeyChecking=accept-new `
                        -o ConnectTimeout=10 -T git@github.com 2>&1
if ($sshOutput -notmatch "successfully authenticated") {
    Write-Host ""
    Write-Host "  SSH authentication to GitHub failed." -ForegroundColor Red
    Write-Host "  Make sure your SSH key is loaded and has access to the private repo."
    Write-Host "  Test manually: ssh -T git@github.com"
    exit 1
}
Write-Success "SSH access confirmed"

# ── Ensure Rust / cargo ───────────────────────────────────────────────────────
$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Warn "cargo not found – installing Rust via rustup..."

    $rustupUrl = "https://win.rustup.rs/x86_64"
    $rustupExe = Join-Path $env:TEMP "rustup-init.exe"
    Write-Info "Downloading rustup from $rustupUrl ..."
    Invoke-WebRequest -Uri $rustupUrl -OutFile $rustupExe -UseBasicParsing
    & $rustupExe -y --no-modify-path
    Remove-Item $rustupExe -Force

    # Add cargo to PATH for this session
    if ($env:PATH -notlike "*$cargoBin*") {
        $env:PATH = "$cargoBin;$env:PATH"
    }
    Write-Success "Rust installed"
} else {
    $rustVer = (& rustc --version)
    Write-Info "Using existing $rustVer"
}

# ── Build and install ─────────────────────────────────────────────────────────
Write-Info "Installing $Binary (compiling from source – may take a few minutes)..."
& cargo install --git $RepoSsh --bin $Binary --locked

Write-Host ""
$installedCmd = Get-Command $Binary -ErrorAction SilentlyContinue
$installedPath = if ($installedCmd) { $installedCmd.Source } else { $null }
if (-not $installedPath) { $installedPath = "$cargoBin\$Binary.exe (may need to restart shell)" }
Write-Success "$Binary installed -> $installedPath"

Write-Host ""
if ($Component -eq "tester") {
    Write-Host "  Quick test:"
    Write-Host "    networker-tester --help"
    Write-Host "    networker-tester --target http://localhost:8080/health --modes http1 --runs 3"
} else {
    Write-Host "  Start the endpoint:"
    Write-Host "    networker-endpoint"
    Write-Host "  (listens on :8080 HTTP, :8443 HTTPS, :9999 UDP)"
}
