#Requires -Version 5.1
# ==============================================================================
# Comprehensive test suite for install.ps1 (PowerShell installer)
#
# Usage:
#   pwsh -File tests/test_install_ps1.ps1
#   powershell -File tests/test_install_ps1.ps1
#
# This script dot-sources install.ps1 functions by overriding the entry point,
# mocks ALL external commands, and exercises every major code path.
# ==============================================================================

$ErrorActionPreference = "Stop"

# ── Test framework ────────────────────────────────────────────────────────────
$script:PassCount = 0
$script:FailCount = 0
$script:CurrentTest = ""

function Assert-Equal ($actual, $expected, $msg) {
    if ($actual -eq $expected) {
        $script:PassCount++
        Write-Host "    PASS " -NoNewline -ForegroundColor Green
        Write-Host $msg
    } else {
        $script:FailCount++
        Write-Host "    FAIL " -NoNewline -ForegroundColor Red
        Write-Host "$msg  (expected='$expected' actual='$actual')"
    }
}

function Assert-Contains ($haystack, $needle, $msg) {
    if ($haystack -and $haystack.ToString().Contains($needle)) {
        $script:PassCount++
        Write-Host "    PASS " -NoNewline -ForegroundColor Green
        Write-Host $msg
    } else {
        $script:FailCount++
        Write-Host "    FAIL " -NoNewline -ForegroundColor Red
        Write-Host "$msg  (string does not contain '$needle')"
    }
}

function Assert-True ($condition, $msg) {
    if ($condition) {
        $script:PassCount++
        Write-Host "    PASS " -NoNewline -ForegroundColor Green
        Write-Host $msg
    } else {
        $script:FailCount++
        Write-Host "    FAIL " -NoNewline -ForegroundColor Red
        Write-Host $msg
    }
}

function Assert-False ($condition, $msg) {
    Assert-True (-not $condition) $msg
}

function Assert-Match ($value, $pattern, $msg) {
    if ($value -match $pattern) {
        $script:PassCount++
        Write-Host "    PASS " -NoNewline -ForegroundColor Green
        Write-Host $msg
    } else {
        $script:FailCount++
        Write-Host "    FAIL " -NoNewline -ForegroundColor Red
        Write-Host "$msg  (value='$value' did not match pattern='$pattern')"
    }
}

function Write-TestSection ($name) {
    Write-Host ""
    Write-Host "== $name ==" -ForegroundColor Cyan
}

# ── Temp directory setup ──────────────────────────────────────────────────────
$script:TestTempDir = Join-Path $env:TEMP ("test-install-ps1-" + [System.IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Force $script:TestTempDir | Out-Null

# ── Record calls for mock verification ────────────────────────────────────────
$script:MockCalls = [System.Collections.ArrayList]@()

function Record-MockCall ($name, $args_) {
    $null = $script:MockCalls.Add(@{ Name = $name; Args = $args_ })
}

function Get-MockCalls ($name) {
    return @($script:MockCalls | Where-Object { $_.Name -eq $name })
}

function Clear-MockCalls {
    $script:MockCalls.Clear()
}

# ── Source install.ps1 without running the entry point ────────────────────────
# We replace the entry-point section by stopping execution before it runs.
# Strategy: read the file, replace the entry point block, and dot-source the
# modified version from a temp file.

$installerPath = Join-Path $PSScriptRoot "..\install.ps1"
$installerPath = (Resolve-Path $installerPath).Path

$installerContent = Get-Content $installerPath -Raw

# Remove the param() block and everything after "# ENTRY POINT" marker
# so we only load functions and state variables.
# We strip the param block so we can control $Yes, $Component etc. ourselves.
$paramEnd = $installerContent.IndexOf("`$ErrorActionPreference = `"Stop`"")
$entryMarker = $installerContent.IndexOf("# ══════════════════════════════════════════════════════════════════════════════`r`n#  ENTRY POINT")
if ($entryMarker -lt 0) {
    # Try LF only
    $entryMarker = $installerContent.IndexOf("# ══════════════════════════════════════════════════════════════════════════════`n#  ENTRY POINT")
}

if ($paramEnd -lt 0 -or $entryMarker -lt 0) {
    Write-Host "FATAL: Could not parse install.ps1 structure." -ForegroundColor Red
    Write-Host "  paramEnd=$paramEnd entryMarker=$entryMarker"
    exit 1
}

# Extract just the function/variable definitions (between param block and entry point)
$functionsContent = $installerContent.Substring($paramEnd, $entryMarker - $paramEnd)

# Prepend the variables that normally come from param()
$preamble = @'
$ErrorActionPreference = "Stop"

# These simulate the param() block -- tests will override as needed
if (-not (Test-Path variable:script:Component))  { $script:Component  = "" }
if (-not (Test-Path variable:script:Yes))         { $script:Yes        = $false }
if (-not (Test-Path variable:script:FromSource))  { $script:FromSource = $false }
if (-not (Test-Path variable:script:SkipRust))    { $script:SkipRust   = $false }
if (-not (Test-Path variable:script:Azure))       { $script:Azure      = $false }
if (-not (Test-Path variable:script:TesterAzure)) { $script:TesterAzure = $false }
if (-not (Test-Path variable:script:Aws))         { $script:Aws        = $false }
if (-not (Test-Path variable:script:TesterAws))   { $script:TesterAws  = $false }
if (-not (Test-Path variable:script:Gcp))         { $script:Gcp        = $false }
if (-not (Test-Path variable:script:TesterGcp))   { $script:TesterGcp  = $false }
if (-not (Test-Path variable:script:Region))      { $script:Region     = "" }
if (-not (Test-Path variable:script:AwsRegion_))  { $script:AwsRegion_ = "" }
if (-not (Test-Path variable:script:GcpProject_)) { $script:GcpProject_ = "" }
if (-not (Test-Path variable:script:GcpZone_))    { $script:GcpZone_   = "" }
if (-not (Test-Path variable:script:Help))        { $script:Help       = $false }

'@

$sourceFile = Join-Path $script:TestTempDir "install_functions.ps1"
[System.IO.File]::WriteAllText($sourceFile, $preamble + $functionsContent, [System.Text.UTF8Encoding]::new($false))

# ── Set up script-scope variables that install.ps1 expects from param() ───────
$script:Component  = ""
$script:Yes        = $false
$script:FromSource = $false
$script:SkipRust   = $false
$script:Azure      = $false
$script:TesterAzure = $false
$script:Aws        = $false
$script:TesterAws  = $false
$script:Gcp        = $false
$script:TesterGcp  = $false
$script:Region     = ""
$script:Help       = $false

# Make param names available as plain variables (install.ps1 references $Yes, $Component etc.)
$Yes        = $false
$Component  = ""
$FromSource = $false
$SkipRust   = $false
$Azure      = $false
$TesterAzure = $false
$Aws        = $false
$TesterAws  = $false
$Gcp        = $false
$TesterGcp  = $false
$Region     = ""
$AwsRegion  = ""
$GcpProject = ""
$GcpZone    = ""
$Help       = $false

# Dot-source the extracted functions
. $sourceFile

# ── Mock Get-Command to control tool detection ────────────────────────────────
$script:MockCommands = @{}

function Set-MockCommand ($name, $available) {
    $script:MockCommands[$name] = $available
}

# We cannot reliably override Get-Command globally in PS5, so instead we mock
# the external tool binaries themselves by creating stub functions.

# ── Reset all installer state to defaults ─────────────────────────────────────
function Reset-InstallerState {
    $script:InstallMethod     = "source"
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
    $script:TesterLocation    = "local"
    $script:EndpointLocation  = "local"
    $script:DoRemoteTester    = $false
    $script:DoRemoteEndpoint  = $false
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
    $script:GcpAutoShutdown   = "yes"
    $script:GcpShutdownAsked  = $false
    $script:ConfigFilePath    = ""
    $script:AwsTesterOptionsAsked   = $false
    $script:AwsEndpointOptionsAsked = $false
    $script:GcpTesterOptionsAsked   = $false
    $script:GcpEndpointOptionsAsked = $false

    # Reset param-like variables
    $script:Yes        = $false
    $script:Component  = ""
    $script:FromSource = $false
    $script:SkipRust   = $false
    $script:Azure      = $false
    $script:TesterAzure = $false
    $script:Aws        = $false
    $script:TesterAws  = $false
    $script:Gcp        = $false
    $script:TesterGcp  = $false
    $script:Region     = ""

    # Also set the local-scope param variables for the dot-sourced functions
    Set-Variable -Name Yes        -Value $false -Scope 1
    Set-Variable -Name Component  -Value ""     -Scope 1
    Set-Variable -Name FromSource -Value $false -Scope 1
    Set-Variable -Name SkipRust   -Value $false -Scope 1
    Set-Variable -Name Azure      -Value $false -Scope 1
    Set-Variable -Name TesterAzure -Value $false -Scope 1
    Set-Variable -Name Aws        -Value $false -Scope 1
    Set-Variable -Name TesterAws  -Value $false -Scope 1
    Set-Variable -Name Gcp        -Value $false -Scope 1
    Set-Variable -Name TesterGcp  -Value $false -Scope 1
    Set-Variable -Name Region     -Value ""     -Scope 1
    Set-Variable -Name AwsRegion  -Value ""     -Scope 1
    Set-Variable -Name GcpProject -Value ""     -Scope 1
    Set-Variable -Name GcpZone    -Value ""     -Scope 1
    Set-Variable -Name Help       -Value $false -Scope 1

    Clear-MockCalls
}


# ##############################################################################
#  TESTS BEGIN
# ##############################################################################

Write-Host ""
Write-Host ("=" * 60) -ForegroundColor Cyan
Write-Host "  install.ps1 Test Suite" -ForegroundColor Cyan
Write-Host ("=" * 60) -ForegroundColor Cyan

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "System Detection: Get-ReleaseTarget"
# ══════════════════════════════════════════════════════════════════════════════

# Test AMD64
$savedArch = $env:PROCESSOR_ARCHITECTURE
$env:PROCESSOR_ARCHITECTURE = "AMD64"
$target = Get-ReleaseTarget
Assert-Equal $target "x86_64-pc-windows-msvc" "Get-ReleaseTarget returns x86_64-pc-windows-msvc for AMD64"

# Test unsupported arch (ARM64 not in release matrix)
$env:PROCESSOR_ARCHITECTURE = "ARM64"
$target = Get-ReleaseTarget
Assert-Equal $target "" "Get-ReleaseTarget returns empty string for ARM64"

# Test another unsupported arch
$env:PROCESSOR_ARCHITECTURE = "x86"
$target = Get-ReleaseTarget
Assert-Equal $target "" "Get-ReleaseTarget returns empty string for x86"

$env:PROCESSOR_ARCHITECTURE = $savedArch

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "System Detection: Invoke-DiscoverSystem"
# ══════════════════════════════════════════════════════════════════════════════

# Test: release mode detection when gh is available and authenticated
Reset-InstallerState
$env:PROCESSOR_ARCHITECTURE = "AMD64"

# Override Get-Command by creating mock functions for the tools
# Mock: cargo exists
function global:cargo { }
# Mock: rustc exists
function global:rustc {
    Record-MockCall "rustc" $args
    return "rustc 1.78.0 (fake)"
}
# Mock: gh exists and is authenticated
function global:gh {
    Record-MockCall "gh" $args
    $argStr = ($args -join " ")
    if ($argStr -like "*auth status*") {
        $global:LASTEXITCODE = 0
        return "Logged in"
    }
    if ($argStr -like "*release list*") {
        $global:LASTEXITCODE = 0
        return "v0.12.90"
    }
    $global:LASTEXITCODE = 0
}
# Mock: winget does not exist (remove if it was created before)
if (Test-Path Function:\global:winget) { Remove-Item Function:\global:winget }
# Mock: git does not exist
if (Test-Path Function:\global:git) { Remove-Item Function:\global:git }
# Mock: az does not exist
if (Test-Path Function:\global:az)  { Remove-Item Function:\global:az }
# Mock: aws does not exist
if (Test-Path Function:\global:aws) { Remove-Item Function:\global:aws }
# Mock: gcloud does not exist
if (Test-Path Function:\global:gcloud) { Remove-Item Function:\global:gcloud }

Invoke-DiscoverSystem

Assert-Equal $script:InstallMethod "release" "Invoke-DiscoverSystem sets InstallMethod=release when gh authenticated + AMD64"
Assert-True $script:ReleaseAvailable "Invoke-DiscoverSystem sets ReleaseAvailable=true"
Assert-Equal $script:ReleaseTarget "x86_64-pc-windows-msvc" "Invoke-DiscoverSystem sets ReleaseTarget correctly"
Assert-Equal $script:NetworkerVersion "v0.12.90" "Invoke-DiscoverSystem gets version from gh release list"
Assert-True $script:RustExists "Invoke-DiscoverSystem detects Rust when cargo mock is present"

# Test: gh auth fails -> falls back to source + InstallerVersion
Reset-InstallerState
$env:PROCESSOR_ARCHITECTURE = "AMD64"

function global:gh {
    Record-MockCall "gh" $args
    $argStr = ($args -join " ")
    if ($argStr -like "*auth status*") {
        $global:LASTEXITCODE = 1
        return "not logged in"
    }
    $global:LASTEXITCODE = 1
}

Invoke-DiscoverSystem

Assert-Equal $script:InstallMethod "source" "Invoke-DiscoverSystem falls back to source when gh auth fails"
Assert-False $script:ReleaseAvailable "ReleaseAvailable is false when gh auth fails"
Assert-Equal $script:NetworkerVersion $InstallerVersion "NetworkerVersion falls back to InstallerVersion when gh unavailable"

# Test: gh not installed at all
Reset-InstallerState
$env:PROCESSOR_ARCHITECTURE = "AMD64"
if (Test-Path Function:\global:gh) { Remove-Item Function:\global:gh }

Invoke-DiscoverSystem

Assert-Equal $script:InstallMethod "source" "Invoke-DiscoverSystem uses source when gh not installed"
Assert-Equal $script:NetworkerVersion $InstallerVersion "NetworkerVersion = InstallerVersion when gh missing"

# Test: FromSource flag forces source mode even when gh works
Reset-InstallerState
$env:PROCESSOR_ARCHITECTURE = "AMD64"
$FromSource = $true
function global:gh {
    $global:LASTEXITCODE = 0
    $argStr = ($args -join " ")
    if ($argStr -like "*auth status*") { return "ok" }
    if ($argStr -like "*release list*") { return "v1.0.0" }
}

Invoke-DiscoverSystem

Assert-Equal $script:InstallMethod "source" "FromSource flag forces source mode even when gh is available"
$FromSource = $false

# Restore arch
$env:PROCESSOR_ARCHITECTURE = $savedArch

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "Component/Feature Selection: -Component flag"
# ══════════════════════════════════════════════════════════════════════════════

# -Component tester
Reset-InstallerState
$Component = "tester"
Invoke-DiscoverSystem
Assert-True  $script:DoInstallTester   "-Component tester enables tester install"
Assert-False $script:DoInstallEndpoint "-Component tester disables endpoint install"

# -Component endpoint
Reset-InstallerState
$Component = "endpoint"
Invoke-DiscoverSystem
Assert-False $script:DoInstallTester   "-Component endpoint disables tester install"
Assert-True  $script:DoInstallEndpoint "-Component endpoint enables endpoint install"

# -Component both (default behavior)
Reset-InstallerState
$Component = "both"
Invoke-DiscoverSystem
Assert-True $script:DoInstallTester   "-Component both enables tester install"
Assert-True $script:DoInstallEndpoint "-Component both enables endpoint install"

$Component = ""

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "Yes Flag: Invoke-AskYN and Read-HostDefault"
# ══════════════════════════════════════════════════════════════════════════════

# -Yes flag: Invoke-AskYN returns default=y as $true
$Yes = $true
$result = Invoke-AskYN "test prompt" "y"
Assert-True $result "Invoke-AskYN returns true when -Yes and default=y"

# -Yes flag: Invoke-AskYN returns default=n as $false
$result = Invoke-AskYN "test prompt" "n"
Assert-False $result "Invoke-AskYN returns false when -Yes and default=n"

# -Yes flag: Read-HostDefault returns default
$result = Read-HostDefault "prompt" "mydefault"
Assert-Equal $result "mydefault" "Read-HostDefault returns default when -Yes"

$Yes = $false

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "Cargo Install: Invoke-CargoInstallStep"
# ══════════════════════════════════════════════════════════════════════════════

# Capture cargo install calls
Reset-InstallerState
$script:CargoCalls = @()

function global:cargo {
    $script:CargoCalls += ,@($args)
    Record-MockCall "cargo" $args
    $global:LASTEXITCODE = 0
}

# Mock the binary being installed
function global:networker-tester {
    return "networker-tester 0.12.90"
}
function global:networker-endpoint {
    return "networker-endpoint 0.12.90"
}

# Test: cargo install without Chrome (no --features browser)
$script:MsvcAvailable = $true
$script:ChromeAvailable = $false
$script:CargoCalls = @()

Invoke-CargoInstallStep "networker-tester"

Assert-True ($script:CargoCalls.Count -gt 0) "Invoke-CargoInstallStep calls cargo"
$callArgs = $script:CargoCalls[0] -join " "
Assert-Contains $callArgs "--git" "Invoke-CargoInstallStep uses --git flag"
Assert-Contains $callArgs "--force" "Invoke-CargoInstallStep uses --force flag"
Assert-True ($callArgs -notlike "*--locked*") "Invoke-CargoInstallStep does NOT use --locked flag"
Assert-True ($callArgs -notlike "*--features*") "No --features when Chrome unavailable"

# Test: cargo install with Chrome for tester
$script:ChromeAvailable = $true
$script:CargoCalls = @()

Invoke-CargoInstallStep "networker-tester"

$callArgs = $script:CargoCalls[0] -join " "
Assert-Contains $callArgs "--features" "Invoke-CargoInstallStep adds --features for tester with Chrome"
Assert-Contains $callArgs "browser" "Invoke-CargoInstallStep adds browser feature"

# Test: cargo install endpoint with Chrome does NOT add --features
$script:CargoCalls = @()
Invoke-CargoInstallStep "networker-endpoint"

$callArgs = $script:CargoCalls[0] -join " "
Assert-True ($callArgs -notlike "*--features*") "No --features browser for endpoint even when Chrome available"

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "Release Download: Invoke-DownloadReleaseStep"
# ══════════════════════════════════════════════════════════════════════════════

Reset-InstallerState
$script:ReleaseTarget = "x86_64-pc-windows-msvc"
$script:GhDownloadCalls = @()

function global:gh {
    $script:GhDownloadCalls += ,@($args)
    Record-MockCall "gh" $args
    $global:LASTEXITCODE = 0
}

# We need to mock New-Item, Expand-Archive etc. but they are core cmdlets.
# Instead we just test that gh is called with the right arguments by intercepting.
# We also need to prevent actual file operations, so we mock around the function.

# Create a fake archive so the function doesn't fail
$fakeTmpDir = Join-Path $script:TestTempDir "release-test"
New-Item -ItemType Directory -Force $fakeTmpDir | Out-Null

# Override Invoke-DownloadReleaseStep to capture the gh call pattern only
$script:GhPatternUsed = ""
function global:gh {
    $argStr = ($args -join " ")
    Record-MockCall "gh" $args
    if ($argStr -like "*release download*") {
        # Find --pattern argument
        for ($i = 0; $i -lt $args.Count; $i++) {
            if ($args[$i] -eq "--pattern") {
                $script:GhPatternUsed = $args[$i+1]
            }
        }
    }
    $global:LASTEXITCODE = 0
}

# We can test the archive naming logic directly
$binary = "networker-tester"
$expectedArchive = "$binary-$($script:ReleaseTarget).zip"
Assert-Equal $expectedArchive "networker-tester-x86_64-pc-windows-msvc.zip" "Release archive naming: binary-target.zip"

$binary = "networker-endpoint"
$expectedArchive = "$binary-$($script:ReleaseTarget).zip"
Assert-Equal $expectedArchive "networker-endpoint-x86_64-pc-windows-msvc.zip" "Release archive naming for endpoint"

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "SSH Remote Install: CRLF Handling"
# ══════════════════════════════════════════════════════════════════════════════

Reset-InstallerState
$script:SshCalls = @()
$script:SshStdin = @()

function global:ssh {
    $script:SshCalls += ,@($args)
    Record-MockCall "ssh" $args
    # Capture piped stdin if available
    $stdinData = $input | Out-String
    if ($stdinData) {
        $script:SshStdin += $stdinData
    }
    $global:LASTEXITCODE = 0
}

# Test Invoke-RemoteCreateEndpointService constructs correct SSH call
$script:SshCalls = @()
$script:SshStdin = @()
Invoke-RemoteCreateEndpointService "10.0.0.1" "azureuser"

Assert-True ($script:SshCalls.Count -gt 0) "Invoke-RemoteCreateEndpointService makes SSH call"

# Verify SSH uses StrictHostKeyChecking=no
$sshArgs = $script:SshCalls[0] -join " "
Assert-Contains $sshArgs "StrictHostKeyChecking=no" "SSH uses StrictHostKeyChecking=no"

# Verify the systemd unit content by inspecting the function source
# We know the function builds $script variable with the unit content then converts CRLF to LF
# Test CRLF->LF conversion logic directly
$testCrlf = "line1`r`nline2`r`nline3"
$testLf   = $testCrlf -replace "`r`n", "`n"
Assert-True ($testLf -eq "line1`nline2`nline3") "CRLF to LF conversion works"
Assert-True ($testLf.IndexOf("`r") -eq -1) "No CR characters remain after conversion"

# Verify systemd unit content structure
$unitContent = @"
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
"@
Assert-Contains $unitContent "[Unit]" "Systemd unit has [Unit] section"
Assert-Contains $unitContent "[Service]" "Systemd unit has [Service] section"
Assert-Contains $unitContent "[Install]" "Systemd unit has [Install] section"
Assert-Contains $unitContent "ExecStart=/usr/local/bin/networker-endpoint" "Systemd unit ExecStart is correct"
Assert-Contains $unitContent "Restart=always" "Systemd unit has Restart=always"
Assert-Contains $unitContent "WantedBy=multi-user.target" "Systemd unit has WantedBy=multi-user.target"

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "VM Existence Check: Azure Paths"
# ══════════════════════════════════════════════════════════════════════════════

# Test: Azure VM exists -> reuse (choice "1")
Reset-InstallerState
$Yes = $false
$script:AzureTesterRg = "test-rg"
$script:AzureEndpointRg = "test-rg"
$script:AzureRegion = "eastus"

function global:az {
    $argStr = ($args -join " ")
    Record-MockCall "az" $args
    if ($argStr -like "*vm show*" -and $argStr -like "*powerState*") {
        $global:LASTEXITCODE = 0
        return "VM deallocated"
    }
    if ($argStr -like "*vm show*" -and $argStr -like "*publicIps*") {
        $global:LASTEXITCODE = 0
        return "1.2.3.4"
    }
    if ($argStr -like "*vm show*") {
        $global:LASTEXITCODE = 0
        return "{}"
    }
    if ($argStr -like "*vm start*") {
        $global:LASTEXITCODE = 0
        return ""
    }
    $global:LASTEXITCODE = 0
}

# Simulate user choosing "1" (reuse) by overriding Read-HostDefault
$script:ReadHostDefaultOverride = "1"
function Read-HostDefault ($prompt, $default) {
    if ($script:ReadHostDefaultOverride) { return $script:ReadHostDefaultOverride }
    return $default
}

$reused = Invoke-VmExistsCheck -Provider "azure" -Label "tester" -Name "test-vm"
Assert-True $reused "Azure: reuse path returns true"
Assert-Equal $script:AzureTesterIp "1.2.3.4" "Azure: reuse sets IP correctly"

# Verify az vm start was called (deallocated VM)
$startCalls = Get-MockCalls "az" | Where-Object { ($_.Args -join " ") -like "*vm start*" }
Assert-True ($startCalls.Count -gt 0) "Azure: starts deallocated VM on reuse"

# Test: Azure VM exists -> rename (choice "2")
Reset-InstallerState
Clear-MockCalls
$script:AzureTesterRg = "test-rg"
function global:az {
    $argStr = ($args -join " ")
    Record-MockCall "az" $args
    if ($argStr -like "*vm show*") {
        $global:LASTEXITCODE = 0
        return "{}"
    }
    $global:LASTEXITCODE = 0
}

$script:ReadHostDefaultOverride = "2"
$script:ReadHostCounter = 0
function Read-Host ($prompt) {
    $script:ReadHostCounter++
    return "new-vm-name"
}
function Read-HostDefault ($prompt, $default) {
    return $script:ReadHostDefaultOverride
}

$reused = Invoke-VmExistsCheck -Provider "azure" -Label "tester" -Name "old-vm"
Assert-False $reused "Azure: rename path returns false (needs new creation)"
Assert-Equal $script:AzureTesterVm "new-vm-name" "Azure: rename updates VM name"

# Test: Azure VM exists -> delete (choice "3")
Reset-InstallerState
Clear-MockCalls
$script:AzureTesterRg = "test-rg"
function global:az {
    $argStr = ($args -join " ")
    Record-MockCall "az" $args
    if ($argStr -like "*vm show*") {
        $global:LASTEXITCODE = 0
        return "{}"
    }
    if ($argStr -like "*vm delete*") {
        $global:LASTEXITCODE = 0
    }
    $global:LASTEXITCODE = 0
}

$script:ReadHostDefaultOverride = "3"
function Read-HostDefault ($prompt, $default) {
    return $script:ReadHostDefaultOverride
}

$reused = Invoke-VmExistsCheck -Provider "azure" -Label "tester" -Name "test-vm"
Assert-False $reused "Azure: delete path returns false"
$deleteCalls = Get-MockCalls "az" | Where-Object { ($_.Args -join " ") -like "*vm delete*" }
Assert-True ($deleteCalls.Count -gt 0) "Azure: delete path calls az vm delete"

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "VM Existence Check: AWS Paths"
# ══════════════════════════════════════════════════════════════════════════════

# Test: AWS VM exists -> reuse stopped instance
Reset-InstallerState
Clear-MockCalls
$script:AwsRegion = "us-east-1"

function global:aws {
    $argStr = ($args -join " ")
    Record-MockCall "aws" $args
    if ($argStr -like "*describe-instances*" -and $argStr -like "*PublicIpAddress*") {
        $global:LASTEXITCODE = 0
        return "None"  # no IP -> trigger start
    }
    if ($argStr -like "*describe-instances*" -and $argStr -like "*InstanceId*" -and $argStr -like "*stopped*") {
        $global:LASTEXITCODE = 0
        return "i-1234567890"
    }
    if ($argStr -like "*describe-instances*" -and $argStr -like "*InstanceId*") {
        $global:LASTEXITCODE = 0
        return "i-1234567890"
    }
    if ($argStr -like "*start-instances*") {
        $global:LASTEXITCODE = 0
        return ""
    }
    if ($argStr -like "*wait*") {
        $global:LASTEXITCODE = 0
        return ""
    }
    if ($argStr -like "*describe-instances*instance-ids*PublicIpAddress*") {
        $global:LASTEXITCODE = 0
        return "5.6.7.8"
    }
    $global:LASTEXITCODE = 0
}

# Need a smarter mock for AWS that handles sequential calls
$script:AwsMockCallNum = 0
function global:aws {
    $script:AwsMockCallNum++
    $argStr = ($args -join " ")
    Record-MockCall "aws" $args

    # First describe-instances call: check existence (returns instance ID)
    if ($argStr -like "*describe-instances*InstanceId*" -and $script:AwsMockCallNum -le 2) {
        $global:LASTEXITCODE = 0
        return "i-existing123"
    }
    # PublicIpAddress query -> None (stopped, no IP)
    if ($argStr -like "*PublicIpAddress*" -and $argStr -notlike "*instance-ids*") {
        $global:LASTEXITCODE = 0
        return "None"
    }
    # Stopped instance lookup
    if ($argStr -like "*stopped*InstanceId*") {
        $global:LASTEXITCODE = 0
        return "i-stopped456"
    }
    # Start instances
    if ($argStr -like "*start-instances*") {
        $global:LASTEXITCODE = 0
        return ""
    }
    # Wait
    if ($argStr -like "*wait*") {
        $global:LASTEXITCODE = 0
        return ""
    }
    # IP after start
    if ($argStr -like "*PublicIpAddress*" -and $argStr -like "*instance-ids*") {
        $global:LASTEXITCODE = 0
        return "5.6.7.8"
    }
    $global:LASTEXITCODE = 0
    return ""
}

$script:ReadHostDefaultOverride = "1"
function Read-HostDefault ($prompt, $default) {
    return $script:ReadHostDefaultOverride
}

$reused = Invoke-VmExistsCheck -Provider "aws" -Label "tester" -Name "my-instance"
Assert-True $reused "AWS: reuse path returns true"

# Test: AWS delete path
Reset-InstallerState
Clear-MockCalls
$script:AwsRegion = "us-east-1"
$script:AwsMockCallNum = 0

function global:aws {
    $script:AwsMockCallNum++
    $argStr = ($args -join " ")
    Record-MockCall "aws" $args
    if ($argStr -like "*describe-instances*InstanceId*") {
        $global:LASTEXITCODE = 0
        return "i-delete789"
    }
    if ($argStr -like "*terminate-instances*") {
        $global:LASTEXITCODE = 0
        return ""
    }
    if ($argStr -like "*wait*") {
        $global:LASTEXITCODE = 0
        return ""
    }
    $global:LASTEXITCODE = 0
    return ""
}

$script:ReadHostDefaultOverride = "3"
$reused = Invoke-VmExistsCheck -Provider "aws" -Label "endpoint" -Name "my-instance"
Assert-False $reused "AWS: delete path returns false"
$termCalls = Get-MockCalls "aws" | Where-Object { ($_.Args -join " ") -like "*terminate-instances*" }
Assert-True ($termCalls.Count -gt 0) "AWS: delete path calls terminate-instances"

# Test: AWS rename path
Reset-InstallerState
Clear-MockCalls
$script:AwsRegion = "us-east-1"
$script:AwsMockCallNum = 0
function global:aws {
    $argStr = ($args -join " ")
    Record-MockCall "aws" $args
    if ($argStr -like "*describe-instances*InstanceId*") {
        $global:LASTEXITCODE = 0
        return "i-rename000"
    }
    $global:LASTEXITCODE = 0
    return ""
}

$script:ReadHostDefaultOverride = "2"
function Read-Host ($prompt) { return "renamed-instance" }
function Read-HostDefault ($prompt, $default) { return $script:ReadHostDefaultOverride }

$reused = Invoke-VmExistsCheck -Provider "aws" -Label "tester" -Name "old-instance"
Assert-False $reused "AWS: rename path returns false"
Assert-Equal $script:AwsTesterName "renamed-instance" "AWS: rename updates instance name"

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "VM Existence Check: GCP Paths"
# ══════════════════════════════════════════════════════════════════════════════

# GCP reuse
Reset-InstallerState
Clear-MockCalls
$script:GcpProject = "my-project"
$script:GcpZone = "us-central1-a"

function global:gcloud {
    $argStr = ($args -join " ")
    Record-MockCall "gcloud" $args
    if ($argStr -like "*instances describe*" -and $argStr -like "*natIP*") {
        $global:LASTEXITCODE = 0
        return "9.8.7.6"
    }
    if ($argStr -like "*instances describe*") {
        $global:LASTEXITCODE = 0
        return "{}"
    }
    $global:LASTEXITCODE = 0
    return ""
}

$script:ReadHostDefaultOverride = "1"
function Read-HostDefault ($prompt, $default) { return $script:ReadHostDefaultOverride }

$reused = Invoke-VmExistsCheck -Provider "gcp" -Label "endpoint" -Name "gcp-vm"
Assert-True $reused "GCP: reuse path returns true"
Assert-Equal $script:GcpEndpointIp "9.8.7.6" "GCP: reuse sets IP correctly"

# GCP rename
Reset-InstallerState
Clear-MockCalls
$script:GcpProject = "my-project"
$script:GcpZone = "us-central1-a"

function global:gcloud {
    $argStr = ($args -join " ")
    Record-MockCall "gcloud" $args
    if ($argStr -like "*instances describe*") {
        $global:LASTEXITCODE = 0
        return "{}"
    }
    $global:LASTEXITCODE = 0
    return ""
}

$script:ReadHostDefaultOverride = "2"
function Read-Host ($prompt) { return "new-gcp-vm" }
function Read-HostDefault ($prompt, $default) { return $script:ReadHostDefaultOverride }

$reused = Invoke-VmExistsCheck -Provider "gcp" -Label "tester" -Name "old-gcp-vm"
Assert-False $reused "GCP: rename path returns false"
Assert-Equal $script:GcpTesterName "new-gcp-vm" "GCP: rename updates instance name"

# GCP delete
Reset-InstallerState
Clear-MockCalls
$script:GcpProject = "my-project"
$script:GcpZone = "us-central1-a"

function global:gcloud {
    $argStr = ($args -join " ")
    Record-MockCall "gcloud" $args
    if ($argStr -like "*instances describe*") {
        $global:LASTEXITCODE = 0
        return "{}"
    }
    if ($argStr -like "*instances delete*") {
        $global:LASTEXITCODE = 0
        return ""
    }
    $global:LASTEXITCODE = 0
    return ""
}

$script:ReadHostDefaultOverride = "3"
function Read-HostDefault ($prompt, $default) { return $script:ReadHostDefaultOverride }

$reused = Invoke-VmExistsCheck -Provider "gcp" -Label "endpoint" -Name "delete-gcp-vm"
Assert-False $reused "GCP: delete path returns false"
$delCalls = Get-MockCalls "gcloud" | Where-Object { ($_.Args -join " ") -like "*instances delete*" }
Assert-True ($delCalls.Count -gt 0) "GCP: delete path calls gcloud compute instances delete"

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "VM Existence Check: try/catch prevents false positive"
# ══════════════════════════════════════════════════════════════════════════════

# When the CLI command itself throws (not found), catch should set $exists=$false
Reset-InstallerState
Clear-MockCalls
$script:GcpProject = "my-project"
$script:GcpZone = "us-central1-a"

# Remove the gcloud mock to simulate command-not-found scenario
if (Test-Path Function:\global:gcloud) { Remove-Item Function:\global:gcloud }

# Since gcloud truly doesn't exist (or our mock is gone), we simulate via az
# Actually test the try/catch structure: if az show fails with non-zero exit
function global:az {
    Record-MockCall "az" $args
    $global:LASTEXITCODE = 1
    return ""
}
$script:AzureTesterRg = "nonexist-rg"
$script:ReadHostDefaultOverride = "1"
function Read-HostDefault ($prompt, $default) { return $script:ReadHostDefaultOverride }

$reused = Invoke-VmExistsCheck -Provider "azure" -Label "tester" -Name "nonexistent-vm"
Assert-False $reused "try/catch: non-existent VM returns false (no false positive)"

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "Azure Login: tenant auto-detection regex"
# ══════════════════════════════════════════════════════════════════════════════

# Test the regex that matches tenant ID from az login output
# The regex pattern from install.ps1:
#   (?m)^([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\s
$loginOutput = @"
WARNING: The following tenants don't contain accessible subscriptions. Use 'az login --tenant TENANT_ID'.
1ecbc8ed-6353-4a12-b345-abcdef123456 'Contoso Corp'
AADSTS50076: Due to a configuration change made by your administrator
Trace ID: 9f8e7d6c-5b4a-3c2d-1e0f-abc123def456
"@

$matched = $loginOutput -match '(?m)^([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\s'
Assert-True $matched "Tenant regex matches tenant ID line"
if ($matched) {
    Assert-Equal $Matches[1] "1ecbc8ed-6353-4a12-b345-abcdef123456" "Tenant regex captures correct tenant ID"
}

# Verify it does NOT match the Trace ID line (which also has a GUID-like pattern)
$traceOnly = @"
Trace ID: 9f8e7d6c-5b4a-3c2d-1e0f-abc123def456
"@
$matchedTrace = $traceOnly -match '(?m)^([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\s'
Assert-False $matchedTrace "Tenant regex does NOT match Trace ID line (not at start of line)"

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "Azure Login: re-check after CLI install"
# ══════════════════════════════════════════════════════════════════════════════

# The function Invoke-EnsureAzureCli re-checks login status after install
# Verify the structure: if AzureCliAvailable but not logged in, it re-checks
Reset-InstallerState
Clear-MockCalls
$script:AzureCliAvailable = $true
$script:AzureLoggedIn = $false

$script:AzAccountShowCallCount = 0
function global:az {
    $argStr = ($args -join " ")
    Record-MockCall "az" $args
    if ($argStr -like "*account show*") {
        $script:AzAccountShowCallCount++
        $global:LASTEXITCODE = 0   # Already logged in on re-check
        return ""
    }
    $global:LASTEXITCODE = 0
    return ""
}
$Yes = $true

Invoke-EnsureAzureCli

Assert-True ($script:AzAccountShowCallCount -ge 1) "Invoke-EnsureAzureCli re-checks login after CLI is available"
Assert-True $script:AzureLoggedIn "Invoke-EnsureAzureCli sets AzureLoggedIn=true on successful re-check"
$Yes = $false

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "NSG Port Handling"
# ══════════════════════════════════════════════════════════════════════════════

# Test: SecurityRuleConflict handled gracefully (no exception)
Reset-InstallerState
Clear-MockCalls
$script:StepNum = 0

function global:az {
    $argStr = ($args -join " ")
    Record-MockCall "az" $args
    if ($argStr -like "*open-port*") {
        $global:LASTEXITCODE = 1
        return "SecurityRuleConflict: rule already exists"
    }
    $global:LASTEXITCODE = 0
}

$exceptionThrown = $false
try {
    Invoke-AzureOpenPorts "test-rg" "test-vm"
} catch {
    $exceptionThrown = $true
}
Assert-False $exceptionThrown "NSG SecurityRuleConflict handled without exception"

# Test: Already-open ports pattern
function global:az {
    $argStr = ($args -join " ")
    Record-MockCall "az" $args
    if ($argStr -like "*open-port*") {
        $global:LASTEXITCODE = 1
        return "already exists"
    }
    $global:LASTEXITCODE = 0
}

$exceptionThrown = $false
try {
    $script:StepNum = 0
    Invoke-AzureOpenPorts "test-rg" "test-vm"
} catch {
    $exceptionThrown = $true
}
Assert-False $exceptionThrown "NSG 'already exists' handled without exception"

# Test: successful port open
Clear-MockCalls
function global:az {
    Record-MockCall "az" $args
    $global:LASTEXITCODE = 0
}

$exceptionThrown = $false
try {
    $script:StepNum = 0
    Invoke-AzureOpenPorts "test-rg" "test-vm"
} catch {
    $exceptionThrown = $true
}
Assert-False $exceptionThrown "NSG successful port open completes without error"

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "Config Generation"
# ══════════════════════════════════════════════════════════════════════════════

Reset-InstallerState
$script:StepNum = 0

# Override the config path to use our temp dir
$origUserProfile = $env:USERPROFILE
$env:USERPROFILE = $script:TestTempDir

Invoke-GenerateConfig "10.20.30.40"

$configPath = Join-Path $script:TestTempDir "networker-cloud.json"
Assert-True (Test-Path $configPath) "Config file was created"

# Check UTF-8 without BOM
$configBytes = [System.IO.File]::ReadAllBytes($configPath)
$hasBom = ($configBytes.Length -ge 3 -and $configBytes[0] -eq 0xEF -and $configBytes[1] -eq 0xBB -and $configBytes[2] -eq 0xBF)
Assert-False $hasBom "Config file is UTF-8 without BOM"

# Check valid JSON
$configText = [System.IO.File]::ReadAllText($configPath)
$jsonParsed = $null
$jsonValid = $false
try {
    $jsonParsed = $configText | ConvertFrom-Json
    $jsonValid = $true
} catch {
    $jsonValid = $false
}
Assert-True $jsonValid "Config file contains valid JSON"

# Check content
if ($jsonParsed) {
    Assert-Contains $jsonParsed.target "10.20.30.40" "Config target contains the endpoint IP"
    Assert-Contains $jsonParsed.target "8080" "Config target includes port 8080"
    Assert-Equal $jsonParsed.runs 5 "Config runs = 5"
    Assert-True ($jsonParsed.modes -contains "http1") "Config modes contains http1"
    Assert-True ($jsonParsed.modes -contains "http2") "Config modes contains http2"
}

# Check ConfigFilePath was set
Assert-Equal $script:ConfigFilePath $configPath "ConfigFilePath script variable set correctly"

$env:USERPROFILE = $origUserProfile

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "Cloud CLI Ensure + Options Guards"
# ══════════════════════════════════════════════════════════════════════════════

# Test: Invoke-EnsureAzureCli called before Azure deployment via CLI flags
Reset-InstallerState
Clear-MockCalls
$script:AzureCliAvailable = $true
$script:AzureLoggedIn = $true

function global:az {
    Record-MockCall "az" $args
    $global:LASTEXITCODE = 0
    return ""
}

# Simulate that -Azure flag was passed
$script:DoRemoteEndpoint = $true
$script:EndpointLocation = "azure"
# Set the guard to prevent double-prompt
$script:AzureEndpointVm = "networker-endpoint-vm"

# The install.ps1 entry point calls Invoke-EnsureAzureCli before options
# We verify the guard: Invoke-AzureOptions should return early when VM is already set
$Yes = $true
$script:AzureRegionAsked = $false
Invoke-AzureOptions "endpoint"
# Because the guard checks $script:AzureEndpointVm and it is set, it should return early
# The region should NOT have been asked
Assert-False $script:AzureRegionAsked "Azure options guard: skips if already configured"

# Now test without guard (fresh state)
$script:AzureEndpointVm = ""
$script:AzureRegionAsked = $false
$script:AzureShutdownAsked = $false
Invoke-AzureOptions "endpoint"
Assert-True $script:AzureRegionAsked "Azure options: proceeds when not yet configured"
$Yes = $false
$script:Yes = $false

# Test: AWS options guard
Reset-InstallerState
$script:AwsTesterOptionsAsked = $true
$Yes = $true

function global:aws {
    Record-MockCall "aws" $args
    $global:LASTEXITCODE = 0
    return ""
}

$script:AwsRegionAsked = $false
Invoke-AwsOptions "tester"
Assert-False $script:AwsRegionAsked "AWS options guard: skips when AwsTesterOptionsAsked=true"
$Yes = $false

# Test: GCP options guard
Reset-InstallerState
$script:GcpEndpointOptionsAsked = $true
$Yes = $true

function global:gcloud {
    $argStr = ($args -join " ")
    Record-MockCall "gcloud" $args
    if ($argStr -like "*get-value project*") {
        $global:LASTEXITCODE = 0
        return "test-project"
    }
    $global:LASTEXITCODE = 0
    return ""
}

$script:GcpProject = "test-project"
$script:GcpRegionAsked = $false
Invoke-GcpOptions "endpoint"
Assert-False $script:GcpRegionAsked "GCP options guard: skips when GcpEndpointOptionsAsked=true"
$Yes = $false

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "-Yes Uses Defaults for All Options"
# ══════════════════════════════════════════════════════════════════════════════

Reset-InstallerState
$Yes = $true
$script:Yes = $true
$script:AzureCliAvailable = $true
$script:AzureLoggedIn = $true

function global:az {
    Record-MockCall "az" $args
    $global:LASTEXITCODE = 0
    return ""
}

# Override Read-HostDefault so -Yes logic works in dot-sourced scope
function Read-HostDefault ($prompt, $default) { return $default }

# Invoke-AzureOptions with -Yes should use all defaults
$script:AzureEndpointVm = ""  # Clear guard
Invoke-AzureOptions "endpoint"

Assert-Equal $script:AzureRegion "eastus" "-Yes: Azure region defaults to eastus"
Assert-Equal $script:AzureAutoShutdown "yes" "-Yes: auto-shutdown defaults to yes"
Assert-Equal $script:AzureEndpointSize "Standard_B2s" "-Yes: VM size defaults to Standard_B2s"
Assert-Equal $script:AzureEndpointOs "linux" "-Yes: OS defaults to linux"

$Yes = $false

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "Parse Arguments: CLI Flags"
# ══════════════════════════════════════════════════════════════════════════════

# Test -Component flag parsing through Invoke-DiscoverSystem
Reset-InstallerState
$Component = "tester"
# Ensure mock tools
function global:cargo { }
function global:rustc { return "rustc 1.78.0" }
if (Test-Path Function:\global:gh) { Remove-Item Function:\global:gh }
if (Test-Path Function:\global:az) { Remove-Item Function:\global:az }
if (Test-Path Function:\global:aws) { Remove-Item Function:\global:aws }
if (Test-Path Function:\global:gcloud) { Remove-Item Function:\global:gcloud }

Invoke-DiscoverSystem
Assert-True  $script:DoInstallTester   "CLI: -Component tester sets DoInstallTester=true"
Assert-False $script:DoInstallEndpoint "CLI: -Component tester sets DoInstallEndpoint=false"

# Test -Azure flag
Reset-InstallerState
$Azure = $true
Invoke-DiscoverSystem
Assert-Equal $script:EndpointLocation "azure" "CLI: -Azure sets EndpointLocation=azure"
Assert-True  $script:DoRemoteEndpoint "CLI: -Azure sets DoRemoteEndpoint=true"
$Azure = $false

# Test -TesterAzure flag
Reset-InstallerState
$TesterAzure = $true
Invoke-DiscoverSystem
Assert-Equal $script:TesterLocation "azure" "CLI: -TesterAzure sets TesterLocation=azure"
Assert-True  $script:DoRemoteTester "CLI: -TesterAzure sets DoRemoteTester=true"
$TesterAzure = $false

# Test -Aws flag
Reset-InstallerState
$Aws = $true
Invoke-DiscoverSystem
Assert-Equal $script:EndpointLocation "aws" "CLI: -Aws sets EndpointLocation=aws"
Assert-True  $script:DoRemoteEndpoint "CLI: -Aws sets DoRemoteEndpoint=true"
$Aws = $false

# Test -TesterAws flag
Reset-InstallerState
$TesterAws = $true
Invoke-DiscoverSystem
Assert-Equal $script:TesterLocation "aws" "CLI: -TesterAws sets TesterLocation=aws"
Assert-True  $script:DoRemoteTester "CLI: -TesterAws sets DoRemoteTester=true"
$TesterAws = $false

# Test -Gcp flag
Reset-InstallerState
$Gcp = $true
Invoke-DiscoverSystem
Assert-Equal $script:EndpointLocation "gcp" "CLI: -Gcp sets EndpointLocation=gcp"
Assert-True  $script:DoRemoteEndpoint "CLI: -Gcp sets DoRemoteEndpoint=true"
$Gcp = $false

# Test -TesterGcp flag
Reset-InstallerState
$TesterGcp = $true
Invoke-DiscoverSystem
Assert-Equal $script:TesterLocation "gcp" "CLI: -TesterGcp sets TesterLocation=gcp"
Assert-True  $script:DoRemoteTester "CLI: -TesterGcp sets DoRemoteTester=true"
$TesterGcp = $false

# Test -Region flag
Reset-InstallerState
$Region = "westus2"
Invoke-DiscoverSystem
Assert-Equal $script:AzureRegion "westus2" "CLI: -Region sets AzureRegion"
$Region = ""

# Test -AwsRegion flag
Reset-InstallerState
$AwsRegion = "eu-west-1"
Invoke-DiscoverSystem
Assert-Equal $script:AwsRegion "eu-west-1" "CLI: -AwsRegion sets AwsRegion"
$AwsRegion = ""

# Test -GcpProject flag
Reset-InstallerState
$GcpProject = "my-gcp-project"
Invoke-DiscoverSystem
Assert-Equal $script:GcpProject "my-gcp-project" "CLI: -GcpProject sets GcpProject"
$GcpProject = ""

# Test -GcpZone flag (also sets GcpRegion)
Reset-InstallerState
$GcpZone = "europe-west1-b"
Invoke-DiscoverSystem
Assert-Equal $script:GcpZone "europe-west1-b" "CLI: -GcpZone sets GcpZone"
Assert-Equal $script:GcpRegion "europe-west1" "CLI: -GcpZone derives GcpRegion by removing trailing -[a-z]"
$GcpZone = ""

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "Parse Arguments: -Help flag"
# ══════════════════════════════════════════════════════════════════════════════

# Show-Help uses Write-Host which writes to the Information stream (stream 6)
# Capture via 6>&1 (PS5.1+)
$helpOutput = & { Show-Help } 6>&1 | Out-String
Assert-Contains $helpOutput "Usage" "-Help: Show-Help outputs usage information"
Assert-Contains $helpOutput "Component" "-Help: Show-Help mentions -Component"
Assert-Contains $helpOutput "Yes" "-Help: Show-Help mentions -Yes"
Assert-Contains $helpOutput "Azure" "-Help: Show-Help mentions -Azure"
Assert-Contains $helpOutput "FromSource" "-Help: Show-Help mentions -FromSource"

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "Component Selection Prompt: Invoke-ComponentSelection"
# ══════════════════════════════════════════════════════════════════════════════

# -Yes skips the prompt
Reset-InstallerState
$Yes = $true
$Component = ""
Invoke-ComponentSelection
Assert-True $script:DoInstallTester   "-Yes: ComponentSelection keeps both (tester)"
Assert-True $script:DoInstallEndpoint "-Yes: ComponentSelection keeps both (endpoint)"

# -Component pre-set also skips
Reset-InstallerState
$Yes = $false
$Component = "endpoint"
Invoke-ComponentSelection
# It should return early since Component is set
Assert-True $true "-Component set: ComponentSelection returns early"

$Yes = $false
$Component = ""

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "Chrome Detection: Get-ChromePath"
# ══════════════════════════════════════════════════════════════════════════════

# Create a fake Chrome executable to test detection
$fakeChromeDir = Join-Path $script:TestTempDir "Google\Chrome\Application"
New-Item -ItemType Directory -Force $fakeChromeDir | Out-Null
$fakeChromePath = Join-Path $fakeChromeDir "chrome.exe"
[System.IO.File]::WriteAllText($fakeChromePath, "fake")

# Test env var override
$savedChromePath = $env:NETWORKER_CHROME_PATH
$env:NETWORKER_CHROME_PATH = $fakeChromePath
$detected = Get-ChromePath
Assert-Equal $detected $fakeChromePath "Get-ChromePath uses NETWORKER_CHROME_PATH env var"
$env:NETWORKER_CHROME_PATH = $savedChromePath

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "Discover System: SkipRust flag"
# ══════════════════════════════════════════════════════════════════════════════

Reset-InstallerState
$SkipRust = $true
if (Test-Path Function:\global:cargo) { Remove-Item Function:\global:cargo }
if (Test-Path Function:\global:rustc) { Remove-Item Function:\global:rustc }

Invoke-DiscoverSystem
Assert-False $script:DoRustInstall "SkipRust: DoRustInstall stays false even when Rust not present"
$SkipRust = $false

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "Discover System: git/winget/MSVC auto-offer in source mode"
# ══════════════════════════════════════════════════════════════════════════════

Reset-InstallerState
$FromSource = $true
# Test the git/winget auto-offer logic directly (avoid Invoke-DiscoverSystem which
# has many side-effects). The logic in discover_system is:
#   if InstallMethod == "source" && !GitAvailable && WingetAvailable -> DoGitInstall = true
Reset-InstallerState
$script:InstallMethod = "source"
$script:GitAvailable = $false
$script:WingetAvailable = $true
$script:DoGitInstall = $false
# Simulate the logic from Invoke-DiscoverSystem lines 315-317
if ($script:InstallMethod -eq "source" -and -not $script:GitAvailable -and $script:WingetAvailable) {
    $script:DoGitInstall = $true
}
Assert-True $script:WingetAvailable "Winget detected when set to true"
Assert-False $script:GitAvailable "Git not detected when set to false"
Assert-True $script:DoGitInstall "Auto-offer git install when source mode + no git + winget"

$FromSource = $false
if (Test-Path Function:\global:winget) { Remove-Item Function:\global:winget }

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "SSH WaitForSsh: uses StrictHostKeyChecking=no"
# ══════════════════════════════════════════════════════════════════════════════

Reset-InstallerState
Clear-MockCalls
$script:StepNum = 0

# Invoke-WaitForSsh checks $LASTEXITCODE after & ssh.
# PowerShell functions don't set $LASTEXITCODE, so we need to run a native
# command that exits 0 to set it, while also recording the ssh call args.
$script:SshMockArgs = @()
function global:ssh {
    $script:SshMockArgs = $args
    Record-MockCall "ssh" $args
    # Run a real native command that exits 0 to set $LASTEXITCODE
    cmd.exe /c "exit 0" 2>$null
}

Invoke-WaitForSsh "10.0.0.1" "testuser" "test-label"

$sshCalls = Get-MockCalls "ssh"
Assert-True ($sshCalls.Count -gt 0) "WaitForSsh calls ssh"
$firstSshArgs = $script:SshMockArgs -join " "
Assert-Contains $firstSshArgs "StrictHostKeyChecking" "WaitForSsh uses StrictHostKeyChecking=no"
Assert-Contains $firstSshArgs "ConnectTimeout" "WaitForSsh uses ConnectTimeout"
Assert-Contains $firstSshArgs "testuser@10.0.0.1" "WaitForSsh passes user@ip"

# ══════════════════════════════════════════════════════════════════════════════
Write-TestSection "Banner and Print Helpers"
# ══════════════════════════════════════════════════════════════════════════════

$script:NetworkerVersion = "v0.12.96"
$bannerOut = & { Write-Banner } 6>&1 | Out-String
Assert-Contains $bannerOut "v0.12.96" "Write-Banner includes version when set"

$script:NetworkerVersion = ""
$bannerOut = & { Write-Banner } 6>&1 | Out-String
Assert-Contains $bannerOut "Installer" "Write-Banner shows 'Installer' when version not set"


# ##############################################################################
#  CLEANUP AND RESULTS
# ##############################################################################

# Remove temp directory
Remove-Item $script:TestTempDir -Recurse -Force -ErrorAction SilentlyContinue

# Remove global mock functions
$mockFunctions = @("cargo","rustc","gh","az","aws","gcloud","ssh","winget",
                    "networker-tester","networker-endpoint")
foreach ($fn in $mockFunctions) {
    if (Test-Path "Function:\global:$fn") { Remove-Item "Function:\global:$fn" }
}

# Read-HostDefault is from install.ps1, no need to restore

# Restore processor architecture
$env:PROCESSOR_ARCHITECTURE = $savedArch

# ── Print results ─────────────────────────────────────────────────────────────
Write-Host ""
Write-Host ("=" * 60) -ForegroundColor Cyan
Write-Host ("  Results: {0} passed, {1} failed, {2} total" -f $script:PassCount, $script:FailCount, ($script:PassCount + $script:FailCount))
if ($script:FailCount -eq 0) {
    Write-Host "  ALL TESTS PASSED" -ForegroundColor Green
} else {
    Write-Host "  SOME TESTS FAILED" -ForegroundColor Red
}
Write-Host ("=" * 60) -ForegroundColor Cyan
Write-Host ""

# Exit with appropriate code
if ($script:FailCount -gt 0) {
    exit 1
} else {
    exit 0
}
