<#
.SYNOPSIS
  Developer environment setup for Windows (the twin of scripts/dev-setup.sh).

.DESCRIPTION
  Checks every prerequisite for working on this repo on Windows 10/11, offers to
  install what is missing with winget (Git, Docker Desktop, Rust via rustup,
  .NET SDK 10, Node.js LTS, jq, CMake, Visual Studio Build Tools), works out the
  machine-specific settings (a busy :5432) and writes them to .dev.env, which
  dev.sh reads. dev.sh itself is bash — run it from Git Bash (installed with Git
  for Windows) or WSL; the Docker lab (lab/lab.sh) works the same way.

.PARAMETER Yes
  Install everything missing without asking.
.PARAMETER Check
  Report only; exit 1 if something is missing.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts/dev-setup.ps1
  powershell -ExecutionPolicy Bypass -File scripts/dev-setup.ps1 -Yes
#>
[CmdletBinding()]
param(
  [switch]$Yes,
  [switch]$Check
)
$ErrorActionPreference = 'Continue'
$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot
$DevEnv = Join-Path $RepoRoot '.dev.env'
$script:Missing = 0

function Ok($m)   { Write-Host "  [ok]   $m" -ForegroundColor Green }
function Warn($m) { Write-Host "  [warn] $m" -ForegroundColor Yellow }
function Bad($m)  { Write-Host "  [x]    $m" -ForegroundColor Red; $script:Missing++ }
function Note($m) { Write-Host "> $m" -ForegroundColor Cyan }
function Ask($q) {
  if ($Yes) { return $true }
  if ($Check) { return $false }
  $r = Read-Host "  $q [Y/n]"
  return -not ($r -match '^(n|no)$')
}
function Have($cmd) { return [bool](Get-Command $cmd -ErrorAction SilentlyContinue) }
function WingetInstall($id, $label) {
  if (-not (Have 'winget')) { Warn "winget not available - install $label manually"; return $false }
  if (Ask "install $label via winget ($id)?") {
    winget install --id $id -e --accept-source-agreements --accept-package-agreements --silent | Out-Null
    # Refresh PATH for this session so the checks below see the new tool.
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User')
    return $true
  }
  return $false
}
function NeedTool($cmd, $label, $wingetId, $url) {
  if (Have $cmd) { Ok "$label ($cmd)"; return }
  Write-Host "  [x]    $label missing" -ForegroundColor Red
  if (-not (WingetInstall $wingetId $label) -or -not (Have $cmd)) {
    if ($url) { Write-Host "         -> $url" }
    $script:Missing++
  } else { Ok "$label installed" }
}
function PortBusy([int]$port) {
  try { $c = New-Object Net.Sockets.TcpClient; $c.Connect('127.0.0.1', $port); $c.Close(); return $true } catch { return $false }
}

Write-Host ""
Write-Host "  networker dev-setup (Windows $([Environment]::OSVersion.Version), $env:PROCESSOR_ARCHITECTURE)" -ForegroundColor Cyan
Write-Host ""

Note "core tools"
NeedTool 'git'  'Git for Windows (includes Git Bash - needed to run dev.sh)' 'Git.Git' 'https://git-scm.com/download/win'
NeedTool 'curl' 'curl' '' ''            # in-box on Windows 10 1803+
NeedTool 'jq'   'jq' 'jqlang.jq' 'https://jqlang.github.io/jq/'
NeedTool 'node' 'Node.js (>= 20)' 'OpenJS.NodeJS.LTS' 'https://nodejs.org'
if (Have 'openssl') { Ok 'openssl' } else { Warn 'openssl not on PATH (Git for Windows ships one at <Git>\usr\bin - Git Bash finds it)' }

Note "docker"
if (Have 'docker') {
  $daemon = $false
  try { docker info *> $null; $daemon = ($LASTEXITCODE -eq 0) } catch {}
  if ($daemon) { Ok "docker $(docker version --format '{{.Server.Version}}' 2>$null) (daemon running)" } else { Bad 'docker installed but Docker Desktop is not running - start it' }
  try { docker compose version *> $null; if ($LASTEXITCODE -eq 0) { Ok 'docker compose v2' } else { Bad 'docker compose v2 missing (Docker Desktop includes it)' } } catch { Bad 'docker compose v2 missing' }
} else {
  Write-Host "  [x]    Docker Desktop missing" -ForegroundColor Red
  if (-not (WingetInstall 'Docker.DockerDesktop' 'Docker Desktop')) { Write-Host '         -> https://docs.docker.com/desktop/install/windows-install/' }
  $script:Missing++
}

Note "rust"
if (Have 'cargo') {
  Ok "$(rustc --version) / $(cargo --version)"
} else {
  Write-Host "  [x]    Rust toolchain missing" -ForegroundColor Red
  if (-not (WingetInstall 'Rustlang.Rustup' 'rustup')) { Write-Host '         -> https://rustup.rs' }
  if (-not (Have 'cargo')) { $script:Missing++ } else { Ok "$(rustc --version)" }
}
# aws-lc-sys (networker-endpoint) needs MSVC + CMake; quinn/h3 build fine with MSVC.
if (Have 'cmake') { Ok 'cmake' } else {
  Write-Host "  [x]    CMake missing (aws-lc-sys build dependency)" -ForegroundColor Red
  if (-not (WingetInstall 'Kitware.CMake' 'CMake')) { Write-Host '         -> https://cmake.org/download/' }
  if (-not (Have 'cmake')) { $script:Missing++ }
}
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$haveMsvc = (Test-Path $vswhere) -and ((& $vswhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null) -ne $null)
if ($haveMsvc) { Ok 'MSVC build tools (link.exe)' } else {
  Warn 'MSVC C++ build tools not found - Rust on Windows needs them (Visual Studio Build Tools, "Desktop development with C++")'
  if (WingetInstall 'Microsoft.VisualStudio.2022.BuildTools' 'Visual Studio 2022 Build Tools') {
    Write-Host '         then add the "Desktop development with C++" workload in the VS Installer'
  }
}

Note ".NET"
if (Have 'dotnet') {
  $v = (dotnet --version 2>$null)
  if ([int]($v.Split('.')[0]) -ge 10) { Ok "dotnet SDK $v" } else { Bad "dotnet SDK $v - need 10.x (https://dotnet.microsoft.com/download)"; WingetInstall 'Microsoft.DotNet.SDK.10' '.NET SDK 10' | Out-Null }
} else {
  Write-Host "  [x]    .NET SDK 10 missing" -ForegroundColor Red
  if (-not (WingetInstall 'Microsoft.DotNet.SDK.10' '.NET SDK 10')) { Write-Host '         -> https://dotnet.microsoft.com/download' }
  if (-not (Have 'dotnet')) { $script:Missing++ } else { Ok "dotnet SDK $(dotnet --version)" }
}

Note "ports"
$pgPort = 5432
if (PortBusy $pgPort) {
  Warn "127.0.0.1:5432 is already in use (a local PostgreSQL?)"
  foreach ($cand in 15432, 25432, 35432, 45432) { if (-not (PortBusy $cand)) { $pgPort = $cand; break } }   # lab.sh uses 55432
  Ok "dev PostgreSQL will use host port $pgPort (written to .dev.env)"
} else { Ok "dev PostgreSQL host port $pgPort" }
foreach ($p in 5030, 5173, 8080) { if (PortBusy $p) { Warn "port $p is in use (control plane 5030 / vite 5173 / endpoint 8080) - dev.sh will try to stop whatever holds it" } }

Note "frontend"
if (Test-Path (Join-Path $RepoRoot 'dashboard\node_modules')) { Ok 'dashboard/node_modules present' }
elseif ((Have 'npm') -and -not $Check -and (Ask 'run npm install in dashboard/ (first time)?')) { Push-Location dashboard; npm install --no-audit --no-fund; Pop-Location }
else { Warn 'dashboard/node_modules missing - cd dashboard; npm install' }

if (-not $Check) {
  @(
    '# Generated by scripts/dev-setup.ps1 - machine-specific dev settings (git-ignored).',
    '# Sourced by dev.sh (run it from Git Bash / WSL), scripts/seed-dev.sh, tests/cli_smoke.sh.',
    "DEV_PG_PORT=$pgPort",
    'DOTNET_BUILD_EXTRA_ARGS=""'
  ) | Set-Content -Path $DevEnv -Encoding ascii
  Ok ".dev.env written (DEV_PG_PORT=$pgPort)"
}

Write-Host ""
if ($script:Missing -gt 0) {
  Write-Host "  [x] $($script:Missing) prerequisite(s) still missing - fix the items above and re-run (a new terminal may be needed for PATH changes)" -ForegroundColor Red
  exit 1
}
Ok 'environment ready'
Write-Host '  next (from Git Bash or WSL, in the repo root):' -ForegroundColor DarkGray
Write-Host '    ./dev.sh                                    endpoint + C# control plane :5030 + vite :5173' -ForegroundColor DarkGray
Write-Host '    ./lab/lab.sh up && ./lab/lab.sh validate    Docker lab: control plane + runners + targets' -ForegroundColor DarkGray
