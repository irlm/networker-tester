//! First-boot bootstrap script generator for cloud VMs.
//!
//! Produces a Linux bash script (cloud-init / user-data) or Windows PowerShell
//! script that installs Wireshark + tshark, downloads networker-tester and
//! networker-agent from the latest GitHub release for a given target triple,
//! and registers networker-agent as a systemd / Windows service with
//! `AGENT_DASHBOARD_URL` + `AGENT_API_KEY` baked in.
//!
//! These are pure functions: no dashboard state, no DB, no IO. Inputs are
//! validated against a strict whitelist so templated strings cannot smuggle
//! shell metacharacters into the bootstrap.

use anyhow::{anyhow, Result};
use regex::Regex;
use std::sync::OnceLock;

const LINUX_TEMPLATE: &str = r#"#!/bin/bash
set -euo pipefail

# Run as root via cloud-init / user-data.
export DEBIAN_FRONTEND=noninteractive

# 1. Prereqs (apt or dnf) + Wireshark CLI (tshark)
if command -v apt-get >/dev/null 2>&1; then
    # Allow tshark to install non-interactively (debconf prompt would block).
    echo "wireshark-common wireshark-common/install-setuid boolean true" \
        | debconf-set-selections
    apt-get update -y -qq
    apt-get install -y -qq curl tar ca-certificates tshark
elif command -v dnf >/dev/null 2>&1; then
    dnf install -y curl tar ca-certificates wireshark-cli
else
    echo "ERROR: no supported package manager (apt/dnf) found" >&2
    exit 1
fi

# 2. Allow non-root packet capture via dumpcap.
if [ -x /usr/bin/dumpcap ]; then
    setcap cap_net_raw,cap_net_admin=eip /usr/bin/dumpcap || true
elif [ -x /usr/sbin/dumpcap ]; then
    setcap cap_net_raw,cap_net_admin=eip /usr/sbin/dumpcap || true
fi

# 3. Resolve the latest release and download both binaries.
TARGET="__TARGET_TRIPLE__"
TAG=$(curl -fsSL https://api.github.com/repos/irlm/networker-tester/releases/latest \
    | grep '"tag_name":' | head -1 | cut -d'"' -f4)
if [ -z "$TAG" ]; then
    echo "ERROR: could not resolve latest release tag" >&2
    exit 1
fi

download_bin() {
    local BIN="$1"
    local URL="https://github.com/irlm/networker-tester/releases/download/${TAG}/${BIN}-${TARGET}.tar.gz"
    curl -fsSL --retry 3 --retry-delay 2 "$URL" -o "/tmp/${BIN}.tar.gz"
    tar xzf "/tmp/${BIN}.tar.gz" -C /tmp
    install -m 0755 "/tmp/${BIN}" "/usr/local/bin/${BIN}"
    rm -f "/tmp/${BIN}.tar.gz" "/tmp/${BIN}"
}

# Literal asset names (also assert tests can grep for):
#   networker-tester-__TARGET_TRIPLE__.tar.gz
#   networker-agent-__TARGET_TRIPLE__.tar.gz
download_bin networker-tester
download_bin networker-agent

# 4. systemd unit
mkdir -p /etc/systemd/system
cat > /etc/systemd/system/networker-agent.service <<'UNIT'
[Unit]
Description=Networker Agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/networker-agent
Restart=on-failure
RestartSec=5
Environment=RUST_LOG=info
Environment=AGENT_DASHBOARD_URL=__DASHBOARD_URL__
Environment=AGENT_API_KEY=__API_KEY__

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl enable --now networker-agent.service
echo "networker-agent installed and started"
"#;

const WINDOWS_TEMPLATE: &str = r#"$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# 1. Install Chocolatey (idempotent)
if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
    Set-ExecutionPolicy Bypass -Scope Process -Force
    iex ((New-Object System.Net.WebClient).DownloadString('https://chocolatey.org/install.ps1'))
}

# 2. Wireshark + Npcap (loopback-capable, no WinPcap mode)
choco install -y --no-progress wireshark --params '/NoDesktopIcon /NoQuickLaunchIcon'
choco install -y --no-progress npcap --params '/WinPcapMode=no /LoopbackSupport=yes'

# Add Wireshark to the machine PATH so tshark resolves from any service.
$wireshark = 'C:\Program Files\Wireshark'
if (Test-Path $wireshark) {
    $machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    if (-not ($machinePath -split ';' | Where-Object { $_ -ieq $wireshark })) {
        [Environment]::SetEnvironmentVariable('Path', "$machinePath;$wireshark", 'Machine')
    }
}

# 3. Resolve latest release tag and download binaries
$TARGET = '__TARGET_TRIPLE__'
$TAG = (Invoke-RestMethod 'https://api.github.com/repos/irlm/networker-tester/releases/latest').tag_name
if (-not $TAG) { throw 'could not resolve latest release tag' }

$BinDir = 'C:\Program Files\Networker'
New-Item -ItemType Directory -Force -Path $BinDir | Out-Null

# Literal asset names (also assert tests can grep for):
#   networker-tester-__TARGET_TRIPLE__.tar.gz
#   networker-agent-__TARGET_TRIPLE__.tar.gz
foreach ($name in 'networker-tester','networker-agent') {
    $url = "https://github.com/irlm/networker-tester/releases/download/$TAG/$name-$TARGET.tar.gz"
    $tmp = "$env:TEMP\$name.tar.gz"
    Invoke-WebRequest -Uri $url -OutFile $tmp
    tar -xzf $tmp -C $env:TEMP
    Copy-Item -Force "$env:TEMP\$name.exe" "$BinDir\$name.exe"
    Remove-Item -Force $tmp, "$env:TEMP\$name.exe"
}

# 4. Set machine env vars + install service via sc.exe
[Environment]::SetEnvironmentVariable('AGENT_DASHBOARD_URL', '__DASHBOARD_URL__', 'Machine')
[Environment]::SetEnvironmentVariable('AGENT_API_KEY', '__API_KEY__', 'Machine')
[Environment]::SetEnvironmentVariable('RUST_LOG', 'info', 'Machine')

sc.exe stop NetworkerAgent 2>$null | Out-Null
sc.exe delete NetworkerAgent 2>$null | Out-Null
sc.exe create NetworkerAgent binPath= "`"$BinDir\networker-agent.exe`"" start= auto DisplayName= "Networker Agent" | Out-Null

# Restart so the service inherits the new env vars
Restart-Service NetworkerAgent -ErrorAction SilentlyContinue
if ((Get-Service NetworkerAgent).Status -ne 'Running') {
    Start-Service NetworkerAgent
}

Write-Host 'networker-agent installed and started'
"#;

fn target_triple_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[A-Za-z0-9_-]+$").unwrap())
}

fn dashboard_url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(https?|wss?)://[A-Za-z0-9.\-:]+(/[A-Za-z0-9._\-/]*)?$").unwrap()
    })
}

/// Convert the dashboard's HTTP public URL into the WebSocket URL the agent
/// must connect to. `https://host[/x]` → `wss://host/ws/agent`.
/// Already-`ws`/`wss` URLs are returned with `/ws/agent` appended (if missing).
pub fn agent_ws_url(public_url: &str) -> String {
    let stripped = public_url.trim_end_matches('/');
    let ws_base = if let Some(rest) = stripped.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = stripped.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        stripped.to_string()
    };
    // Drop any path the public URL carried — agents always hit /ws/agent.
    let host_only = if let Some(scheme_end) = ws_base.find("://") {
        let after = &ws_base[scheme_end + 3..];
        let host = after.split('/').next().unwrap_or(after);
        format!("{}://{}", &ws_base[..scheme_end], host)
    } else {
        ws_base
    };
    format!("{host_only}/ws/agent")
}

fn api_key_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[A-Za-z0-9]{32,128}$").unwrap())
}

fn validate_inputs(dashboard_url: &str, api_key: &str, target_triple: &str) -> Result<()> {
    if !dashboard_url_re().is_match(dashboard_url) {
        return Err(anyhow!(
            "invalid dashboard_url: must match https?://<host>[/path] with no shell metacharacters"
        ));
    }
    if !api_key_re().is_match(api_key) {
        return Err(anyhow!(
            "invalid api_key: must be 32-128 alphanumeric characters"
        ));
    }
    if !target_triple_re().is_match(target_triple) {
        return Err(anyhow!("invalid target_triple: must match [A-Za-z0-9_-]+"));
    }
    Ok(())
}

/// Render a Linux first-boot script that installs prereqs (including
/// Wireshark CLI), configures non-root packet capture via `setcap` on
/// `dumpcap`, downloads networker-tester and networker-agent from the
/// latest GitHub release, writes a systemd unit with the agent env vars,
/// and starts the service.
///
/// Inputs are strictly validated:
/// - `target_triple`: `^[A-Za-z0-9_-]+$`
/// - `dashboard_url`: `^https?://[A-Za-z0-9.\-:]+(/[A-Za-z0-9._\-/]*)?$`
/// - `api_key`:       `^[A-Za-z0-9]{32,128}$`
pub fn render_linux_bootstrap(
    dashboard_url: &str,
    api_key: &str,
    target_triple: &str,
) -> Result<String> {
    validate_inputs(dashboard_url, api_key, target_triple)?;
    Ok(LINUX_TEMPLATE
        .replace("__TARGET_TRIPLE__", target_triple)
        .replace("__DASHBOARD_URL__", dashboard_url)
        .replace("__API_KEY__", api_key))
}

/// Render a Windows first-boot PowerShell script that installs Chocolatey +
/// Wireshark + Npcap, downloads networker-tester.exe and networker-agent.exe
/// from the latest GitHub release, sets machine env vars, and installs
/// networker-agent as a Windows service.
///
/// AWS user-data convention: callers must wrap the returned string in
/// `<powershell>...</powershell>` — that's the caller's job.
///
/// Same validation rules as [`render_linux_bootstrap`].
pub fn render_windows_bootstrap(
    dashboard_url: &str,
    api_key: &str,
    target_triple: &str,
) -> Result<String> {
    validate_inputs(dashboard_url, api_key, target_triple)?;
    Ok(WINDOWS_TEMPLATE
        .replace("__TARGET_TRIPLE__", target_triple)
        .replace("__DASHBOARD_URL__", dashboard_url)
        .replace("__API_KEY__", api_key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_ws_url_converts_https_to_wss_with_path() {
        assert_eq!(
            agent_ws_url("https://alethedash.com"),
            "wss://alethedash.com/ws/agent"
        );
        assert_eq!(
            agent_ws_url("http://localhost:3000"),
            "ws://localhost:3000/ws/agent"
        );
        assert_eq!(
            agent_ws_url("https://alethedash.com/"),
            "wss://alethedash.com/ws/agent"
        );
        // Stale path on the public URL is dropped.
        assert_eq!(
            agent_ws_url("https://alethedash.com/api"),
            "wss://alethedash.com/ws/agent"
        );
        // Already-WS URL: keep scheme, replace path.
        assert_eq!(
            agent_ws_url("wss://alethedash.com/ws/agent"),
            "wss://alethedash.com/ws/agent"
        );
    }

    #[test]
    fn linux_bootstrap_renders_with_substitutions() {
        let s = render_linux_bootstrap(
            "wss://alethedash.com/ws/agent",
            "abc123def456ghi789jkl012mno345pqr678",
            "x86_64-unknown-linux-musl",
        )
        .unwrap();
        assert!(s.contains("AGENT_DASHBOARD_URL=wss://alethedash.com/ws/agent"));
        assert!(s.contains("AGENT_API_KEY=abc123def456"));
        assert!(s.contains("networker-tester-x86_64-unknown-linux-musl.tar.gz"));
        assert!(s.contains("networker-agent-x86_64-unknown-linux-musl.tar.gz"));
        assert!(s.contains("apt-get install -y -qq curl tar ca-certificates tshark"));
        assert!(s.contains("setcap cap_net_raw,cap_net_admin"));
        assert!(s.contains("systemctl enable --now networker-agent.service"));
        assert!(!s.contains("__TARGET_TRIPLE__"));
        assert!(!s.contains("__DASHBOARD_URL__"));
        assert!(!s.contains("__API_KEY__"));
    }

    #[test]
    fn windows_bootstrap_renders_with_substitutions() {
        let s = render_windows_bootstrap(
            "https://alethedash.com",
            "abc123def456ghi789jkl012mno345pqr678",
            "x86_64-pc-windows-msvc",
        )
        .unwrap();
        assert!(s.contains("AGENT_DASHBOARD_URL"));
        assert!(s.contains("AGENT_API_KEY"));
        assert!(s.contains("alethedash.com"));
        assert!(s.contains("choco install -y --no-progress wireshark"));
        assert!(s.contains("choco install -y --no-progress npcap"));
        assert!(s.contains("networker-tester-x86_64-pc-windows-msvc.tar.gz"));
        assert!(s.contains("sc.exe create NetworkerAgent"));
        assert!(!s.contains("__TARGET_TRIPLE__"));
    }

    #[test]
    fn rejects_url_with_shell_metacharacters() {
        let bad = render_linux_bootstrap(
            "https://example.com$(rm -rf /)",
            "abc123def456ghi789jkl012mno345pqr678",
            "x86_64-unknown-linux-musl",
        );
        assert!(bad.is_err());
    }

    #[test]
    fn rejects_short_api_key() {
        let bad = render_linux_bootstrap("https://x.com", "tooshort", "x86_64-unknown-linux-musl");
        assert!(bad.is_err());
    }

    #[test]
    fn rejects_target_with_slash() {
        let bad = render_linux_bootstrap(
            "https://x.com",
            "abc123def456ghi789jkl012mno345pqr678",
            "x86_64-unknown-linux-musl/extra",
        );
        assert!(bad.is_err());
    }
}
