#Requires -Version 5.1
# ──────────────────────────────────────────────────────────────────────────────
# Networker Tester – Windows interactive installer (rustup-style)
#
# Two install modes (auto-detected, or choose in customize flow):
#   release  – download pre-built binary from the latest GitHub release via
#              gh CLI (fast, ~10 s); requires: gh installed + gh auth login
#   source   – compile from source via cargo install (slower, ~5-10 min);
#              requires: SSH key for the private repo + Rust/cargo
#
# Usage (piped):
#   irm <raw-gist-url>/install.ps1 | iex
#
# Usage (downloaded):
#   .\install.ps1 [-Component tester|endpoint|both] [-Yes] [-FromSource]
#                 [-SkipSshCheck] [-SkipRust] [-Help]
#
# Prerequisites (source mode):
#   - Git for Windows (includes ssh.exe) – https://git-scm.com/
#   - SSH key configured for github.com in %USERPROFILE%\.ssh\
# ──────────────────────────────────────────────────────────────────────────────
param(
    [string]$Component  = "both",
    [switch]$Yes,
    [switch]$FromSource,
    [switch]$SkipSshCheck,
    [switch]$SkipRust,
    [switch]$Help
)

$ErrorActionPreference = "Stop"

$ScriptVersion = "0.12.15"
$RepoSsh       = "ssh://git@github.com/irlm/networker-tester"
$RepoGh        = "irlm/networker-tester"
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
    Write-Host "Install modes (auto-detected; override in customize flow or via flag):"
    Write-Host "  release   Download pre-built binary via gh CLI -- fast (~10 s)"
    Write-Host "            Requires: gh installed and authenticated (gh auth login)"
    Write-Host "  source    Compile from private Git repo via cargo install -- slower"
    Write-Host "            Requires: SSH key for github.com + Rust/cargo"
    Write-Host ""
    Write-Host "  -Yes           Non-interactive: accept all defaults"
    Write-Host "  -FromSource    Force source-compile mode (skip release detection)"
    Write-Host "  -SkipSshCheck  Skip the GitHub SSH connectivity test (source mode)"
    Write-Host "  -SkipRust      Skip Rust installation (source mode)"
    Write-Host "  -Help          Show this help message"
    Write-Host ""
    Write-Host "Examples:"
    Write-Host "  .\install.ps1 -Component tester"
    Write-Host "  .\install.ps1 -Yes -Component endpoint"
    Write-Host "  .\install.ps1 -FromSource -SkipRust -Component both"
}

# ── Script-level state ($script: prefix required to mutate from inside functions)
$script:InstallMethod     = "source"   # "release" | "source"
$script:ReleaseAvailable  = $false
$script:ReleaseTarget     = ""
$script:DoSshCheck        = $true
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
$script:SysOs             = ""
$script:SysArch           = ""
$script:StepNum           = 0

# ── Target triple detection ────────────────────────────────────────────────────
function Get-ReleaseTarget {
    switch ($env:PROCESSOR_ARCHITECTURE) {
        "AMD64"  { return "x86_64-pc-windows-msvc" }
        default  { return "" }   # ARM64/x86 not yet in release matrix
    }
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
    if ($SkipSshCheck) { $script:DoSshCheck = $false }

    # Git + winget detection (winget checked regardless of git status)
    $script:GitAvailable    = $null -ne (Get-Command git    -ErrorAction SilentlyContinue)
    $script:WingetAvailable = $null -ne (Get-Command winget -ErrorAction SilentlyContinue)

    # MSVC C++ Build Tools detection (required by Rust's default msvc target)
    # vswhere.exe is the authoritative way to query Visual Studio installations.
    $vswhereExe = Join-Path ([System.Environment]::GetFolderPath('ProgramFilesX86')) `
                             "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhereExe) {
        $vsPath = & $vswhereExe -latest -products * `
            -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
            -property installationPath 2>&1
        $script:MsvcAvailable = -not [string]::IsNullOrWhiteSpace($vsPath)
    } else {
        # vswhere absent → VS not installed; check for link.exe on PATH as fallback
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
            }
        }
    }

    # Auto-offer git install only in source mode (release mode uses gh, not git)
    if ($script:InstallMethod -eq "source" -and -not $script:GitAvailable -and $script:WingetAvailable) {
        $script:DoGitInstall = $true
    }

    # Auto-offer MSVC install only in source mode
    if ($script:InstallMethod -eq "source" -and -not $script:MsvcAvailable -and $script:WingetAvailable) {
        $script:DoMsvcInstall = $true
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
    Write-Host ("    {0,-22} {1}" -f "Install to:",   $CargoBin)
    if ($script:ReleaseAvailable) {
        Write-Host ("    {0,-22} {1}" -f "gh CLI:", "authenticated v")
    }
}

function Show-Plan {
    Write-Section "Installation Plan"
    Write-Host ""
    $step = 1

    if ($script:InstallMethod -eq "release") {
        Write-Host "    Method:  Download binary from GitHub release  (fast)" -ForegroundColor White
        Write-Host ("    Target:  " + $script:ReleaseTarget) -ForegroundColor DarkGray
        Write-Host ""
        if ($script:DoInstallTester) {
            Write-Host ("    {0}. Download networker-tester    gh release download (latest)" -f $step)
            $step++
        }
        if ($script:DoInstallEndpoint) {
            Write-Host ("    {0}. Download networker-endpoint  gh release download (latest)" -f $step)
            $step++
        }
        Write-Host ""
        Write-Dim "Repository:  $RepoGh  (latest release)"
    } else {
        Write-Host "    Method:  Compile from source  (~5-10 min)" -ForegroundColor White
        Write-Host ""

        if (-not $script:GitAvailable) {
            if ($script:DoGitInstall) {
                Write-Host ("    {0}. Install git            Install via winget" -f $step)
                $step++
            } elseif ($script:WingetAvailable) {
                Write-Host "    -. Install git            (skip -- toggle in Customize)" -ForegroundColor DarkGray
            } else {
                Write-Host "    -. Install git            (not installed -- visit https://git-scm.com/)" -ForegroundColor DarkGray
            }
        }

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
        if (-not $script:MsvcAvailable) {
            if ($script:DoMsvcInstall) {
                Write-Host ("    {0}. Install VC++ Build Tools  winget install  (~2-3 GB, 5-15 min)" -f $step)
                $step++
            } elseif ($script:WingetAvailable) {
                Write-Host "    -. Install VC++ Build Tools  (skip -- toggle in Customize)" -ForegroundColor DarkGray
            } else {
                Write-Host "    -. Install VC++ Build Tools  (not installed -- https://aka.ms/vs/buildtools)" -ForegroundColor DarkGray
            }
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

    if ($script:ReleaseAvailable) {
        Write-Host "  Install method:"
        Write-Host "    1) Download binary from latest release  (fast, recommended)"
        Write-Host "    2) Compile from source  (requires SSH key + Rust)"
        Write-Host ""
        $methodAns = Read-Host "  Choice [1]"
        if ([string]::IsNullOrWhiteSpace($methodAns)) { $methodAns = "1" }
        switch ($methodAns.Trim()) {
            "2"     { $script:InstallMethod = "source"  }
            default { $script:InstallMethod = "release" }
        }
        Write-Host ""
    }

    if ($script:InstallMethod -eq "source") {
        # git install (only offered when git is absent)
        if (-not $script:GitAvailable) {
            if ($script:WingetAvailable) {
                Write-Host ""
                $script:DoGitInstall = Invoke-AskYN "git is not installed -- install it via winget?" "y"
                if (-not $script:DoGitInstall) {
                    Write-Host ""
                    Write-Warn "Skipping git -- cargo will use its built-in libgit2 for SSH."
                    Write-Warn "If authentication fails, install git from: https://git-scm.com/"
                }
            } else {
                Write-Host ""
                Write-Warn "git is not installed and winget was not found."
                Write-Host "  Install Git from: https://git-scm.com/"
                Write-Host "  Then re-run this installer."
            }
            Write-Host ""
        }

        # MSVC Build Tools offer (only when absent in source mode)
        if (-not $script:MsvcAvailable) {
            if ($script:WingetAvailable) {
                Write-Host ""
                $script:DoMsvcInstall = Invoke-AskYN "VC++ Build Tools not found -- install via winget? (~2-3 GB)" "y"
                if (-not $script:DoMsvcInstall) {
                    Write-Host ""
                    Write-Warn "Skipping -- cargo will fail with 'linker not found' without MSVC tools."
                    Write-Host "  Install manually from: https://aka.ms/vs/buildtools"
                    Write-Host "  Select 'Desktop development with C++' workload, then re-run."
                }
            } else {
                Write-Host ""
                Write-Warn "VC++ Build Tools are not installed (winget not found to auto-install)."
                Write-Host "  Install from: https://aka.ms/vs/buildtools"
                Write-Host "  Select 'Desktop development with C++' workload, then re-run."
            }
            Write-Host ""
        }

        $script:DoSshCheck = Invoke-AskYN "Run SSH connectivity check for GitHub?" "y"
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
        Write-Host ""
    }

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

# ── Release-mode steps ─────────────────────────────────────────────────────────
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
        Write-Host "  Check releases: gh release list --repo $RepoGh"
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

# ── Source-mode steps ──────────────────────────────────────────────────────────
function Invoke-MsvcInstallStep {
    Invoke-NextStep "Install Visual C++ Build Tools"
    Write-Info "Installing MSVC build tools via winget..."
    Write-Dim "Includes the C++ linker (link.exe) required to compile Rust on Windows."
    Write-Dim "Download: ~2-3 GB  |  Estimated time: 5-15 minutes."
    Write-Host ""

    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & winget install --id Microsoft.VisualStudio.2022.BuildTools -e --source winget `
        --accept-package-agreements --accept-source-agreements `
        --override "--quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $prevErr

    if ($exitCode -ne 0) {
        Write-Host ""
        Write-Err "winget install failed (exit code $exitCode)."
        Write-Host "  Install manually from: https://aka.ms/vs/buildtools"
        Write-Host "  Select the 'Desktop development with C++' workload, then re-run."
        exit 1
    }

    # Source vcvars64.bat so the MSVC toolchain is available in this session
    $vswhereExe = Join-Path ([System.Environment]::GetFolderPath('ProgramFilesX86')) `
                             "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhereExe) {
        $vsPath = (& $vswhereExe -latest -products * `
            -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
            -property installationPath 2>&1).Trim()
        if ($vsPath) {
            $vcvars = Join-Path $vsPath "VC\Auxiliary\Build\vcvars64.bat"
            if (Test-Path $vcvars) {
                Write-Info "Loading MSVC environment (vcvars64.bat)..."
                # Invoke vcvars inside cmd.exe and capture resulting env vars
                $envOutput = cmd.exe /c "`"$vcvars`" > NUL 2>&1 && set" 2>&1
                foreach ($line in $envOutput) {
                    if ($line -match "^([^=]+)=(.+)$") {
                        [System.Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], "Process")
                    }
                }
                $script:MsvcAvailable = $true
                Write-Ok "VC++ Build Tools installed and loaded for this session"
                return
            }
        }
    }
    Write-Warn "Build Tools installed -- PATH changes take effect after reopening your terminal."
    Write-Warn "If cargo still fails, close this window and re-run the installer."
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
        Write-Host ""
        Write-Err "winget install failed (exit code $exitCode)."
        Write-Host "  Install Git manually from: https://git-scm.com/"
        Write-Host "  Then re-run this installer."
        exit 1
    }

    # Refresh PATH so git.exe is visible without reopening the terminal
    $machinePath = [System.Environment]::GetEnvironmentVariable("PATH", "Machine")
    $userPath    = [System.Environment]::GetEnvironmentVariable("PATH", "User")
    $env:PATH    = "$machinePath;$userPath"

    $gitCmd = Get-Command git -ErrorAction SilentlyContinue
    if ($gitCmd) {
        $script:GitAvailable = $true
        Write-Ok ("git installed: " + (& git --version 2>&1))
    } else {
        Write-Warn "git was installed but is not yet in PATH."
        Write-Warn "You may need to reopen your terminal. Continuing with built-in libgit2..."
        $script:GitAvailable = $false
    }
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
        Write-Host "  If Rust was just installed, open a new terminal and re-run this script."
        exit 1
    }
}

function Invoke-CargoInstallStep ($binary) {
    Invoke-NextStep "Install $binary"
    Write-Info "Building and installing $binary from source..."
    Write-Dim "This compiles from the private Git repo and may take a few minutes."

    # Pre-flight: warn if MSVC linker is still absent (user skipped install or no winget)
    if (-not $script:MsvcAvailable) {
        Write-Host ""
        Write-Warn "VC++ Build Tools not detected -- cargo will likely fail with 'linker not found'."
        Write-Host "  To install now, run:"
        Write-Host "    winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override `"--quiet --add Microsoft.VisualStudio.Workload.VCTools`""
        Write-Host "  Then re-run this installer."
        Write-Host ""
    } else {
        Write-Host ""
    }

    # CARGO_NET_GIT_FETCH_WITH_CLI=true delegates git operations to the system
    # git binary rather than libgit2, which reliably picks up the SSH agent.
    # Only set it when git.exe is actually on PATH; if git is absent, cargo falls
    # back to its built-in libgit2 (which reads keys from %USERPROFILE%\.ssh\).
    $gitAvailable = $null -ne (Get-Command git -ErrorAction SilentlyContinue)
    if ($gitAvailable) { $env:CARGO_NET_GIT_FETCH_WITH_CLI = "true" }

    $prevErr = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & cargo install --git $RepoSsh $binary --locked --force
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $prevErr

    if ($gitAvailable) { $env:CARGO_NET_GIT_FETCH_WITH_CLI = $null }

    if ($exitCode -ne 0) {
        Write-Host ""
        Write-Err "cargo install failed (exit code $exitCode)."
        if (-not $gitAvailable) {
            Write-Host "  git.exe was not found -- cargo used built-in libgit2 for SSH."
            Write-Host "  If the error is SSH-related, install Git for Windows and retry:"
            Write-Host "    https://git-scm.com/"
        }
        exit 1
    }

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

    if ($env:PATH -notlike "*$CargoBin*") {
        Write-Warn "$CargoBin is not in PATH for this session."
        Write-Host ""
        Write-Host "  Run now:"
        Write-Host ('    $env:PATH = "' + $CargoBin + ';$env:PATH"')
        Write-Host ""
        Write-Host "  Make permanent (User scope):"
        Write-Host ('    [Environment]::SetEnvironmentVariable("PATH","' + $CargoBin + ';$env:PATH","User")')
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

if ($Component -notin @("tester", "endpoint", "both")) {
    Write-Err "Invalid -Component value '$Component'. Use: tester, endpoint, or both."
    exit 1
}

Invoke-DiscoverSystem

Write-Banner
Show-SystemInfo
Show-Plan
Invoke-MainPrompt

if ($script:InstallMethod -eq "release") {
    New-Item -ItemType Directory -Force $CargoBin | Out-Null
    if ($script:DoInstallTester)   { Invoke-DownloadReleaseStep "networker-tester" }
    if ($script:DoInstallEndpoint) { Invoke-DownloadReleaseStep "networker-endpoint" }
} else {
    if ($script:DoMsvcInstall)     { Invoke-MsvcInstallStep }
    if ($script:DoGitInstall)      { Invoke-GitInstallStep }
    if ($script:DoSshCheck)        { Invoke-SshStep }
    if ($script:DoRustInstall)     { Invoke-RustInstallStep }
    Invoke-EnsureCargoEnv
    if ($script:DoInstallTester)   { Invoke-CargoInstallStep "networker-tester" }
    if ($script:DoInstallEndpoint) { Invoke-CargoInstallStep "networker-endpoint" }
}

Show-Completion
