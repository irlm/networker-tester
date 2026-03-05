# Installation

---

## Quick Install (recommended)

The installer uses a **rustup-style interactive UX**: it shows a system info table, a numbered plan, and a `1) Proceed / 2) Customize / 3) Cancel` prompt before doing anything.

### macOS and Linux

```bash
# Install the diagnostic CLI (networker-tester)
curl -fsSL https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.sh | bash -s -- tester

# Install the test server (networker-endpoint)
curl -fsSL https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.sh | bash -s -- endpoint

# Install both on the same machine
curl -fsSL https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.sh | bash -s -- both
```

### Windows (PowerShell)

```powershell
$GistUrl = 'https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.ps1'

# Install the diagnostic CLI (default)
Invoke-RestMethod $GistUrl | Invoke-Expression

# Install the test server
Invoke-WebRequest $GistUrl -OutFile "$env:TEMP\networker-install.ps1"
& "$env:TEMP\networker-install.ps1" -Component endpoint
```

Compatible with Windows PowerShell 5.1 and PowerShell 7+.

---

## What the Installer Does

The installer auto-detects the fastest available install method:

| Mode | When used | Speed |
|------|-----------|-------|
| **Release** | `gh` CLI is installed and authenticated | ~10 seconds |
| **Source** | `gh` not available; compiles from GitHub repo via `cargo install` | 2–10 minutes |

In source mode, the installer also offers to install missing dependencies:
- Git (via `brew` / `apt-get` / `dnf` / `pacman` / `zypper` / `apk` / `winget`)
- Visual C++ Build Tools on Windows (via `winget`)
- Rust (via [rustup](https://rustup.rs/))

---

## Local Install Options

### Non-interactive (accept all defaults)

```bash
bash install.sh -y tester
bash install.sh -y endpoint
```

### Force source compile

```bash
bash install.sh --from-source tester
```

### Skip Rust installation check

```bash
bash install.sh --skip-rust tester
```

---

## Cloud Deployment

The installer can provision a VM and deploy the endpoint (or tester) to it automatically.

### Azure

```bash
# Deploy endpoint to Azure VM (interactive — picks defaults, shows plan)
bash install.sh --azure endpoint

# With explicit options
bash install.sh --azure endpoint \
  --region eastus \
  --rg my-resource-group \
  --vm my-endpoint-vm \
  --vm-size Standard_B2s
```

### AWS

```bash
# Deploy endpoint to AWS EC2 (interactive)
bash install.sh --aws endpoint

# With explicit options
bash install.sh --aws endpoint \
  --aws-region us-east-1 \
  --aws-instance-type t3.small \
  --aws-endpoint-name networker-endpoint
```

See [[Cloud-Deployment]] for a full walkthrough.

---

## Build from Source

```bash
git clone https://github.com/irlm/networker-tester.git
cd networker-tester
cargo build --release
# Binaries at: target/release/networker-tester
#              target/release/networker-endpoint
```

### With browser probe support

```bash
cargo build --release --features browser -p networker-tester
```

Requires Chrome or Chromium installed locally.

---

## Upgrading

Re-run the same install command on every machine where the binary is used. The installer always downloads or compiles the latest release.

---

## Requirements

| Component | Requirement |
|-----------|------------|
| macOS | 11+ (Apple Silicon or Intel) |
| Linux | Any modern distribution (glibc or musl) |
| Windows | Windows 10+ with PowerShell 5.1 or 7+ |
| HTTP/3 | Included by default (UDP port 8443 must not be firewalled) |
| Browser probe | Chrome or Chromium installed; build with `--features browser` |
| Source compile | Rust stable toolchain via [rustup](https://rustup.rs/) |
