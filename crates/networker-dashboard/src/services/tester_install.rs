//! Install networker-tester + networker-agent binaries on a freshly-provisioned tester VM.
//!
//! This downloads pre-built binaries from GitHub releases — NO source
//! compilation. The previous version cloned the repo + ran `npm install`
//! which took 20+ minutes on small VMs. This version takes ~30 seconds.
//!
//! Chrome + chrome-harness (for browser benchmarks) is an optional step
//! that only runs when `install_chrome_harness` is true.

#![allow(dead_code)]

use anyhow::{anyhow, Context};
use uuid::Uuid;

/// Minimal view of a persistent tester needed to run the install.
#[derive(Debug, Clone)]
pub struct TesterTarget {
    pub tester_id: Uuid,
    pub public_ip: Option<String>,
    pub ssh_user: String,
}

/// Release version to install. Uses the dashboard's own version so testers
/// match the dashboard that provisioned them.
fn release_tag() -> String {
    format!("v{}", env!("CARGO_PKG_VERSION"))
}

/// Detected tester OS info.
#[derive(Debug, Clone)]
pub struct TesterOsInfo {
    /// e.g. "ubuntu", "debian", "amazonlinux"
    pub distro: String,
    /// e.g. "24.04", "22.04"
    pub version: String,
    /// "desktop" | "server" | "minimal"
    pub variant: String,
    /// "x86_64" | "aarch64"
    pub arch: String,
    /// Kernel version
    pub kernel: String,
}

impl TesterOsInfo {
    /// Target triple for downloading pre-built binaries.
    pub fn release_target(&self) -> &'static str {
        match (self.arch.as_str(), self.distro.as_str()) {
            ("x86_64", _) => "x86_64-unknown-linux-musl",
            ("aarch64", _) => "aarch64-unknown-linux-musl",
            _ => "x86_64-unknown-linux-musl",
        }
    }

    /// Human-readable label for the UI (e.g. "Ubuntu 24.04 Server (x86_64)").
    pub fn label(&self) -> String {
        let distro = match self.distro.as_str() {
            "ubuntu" => "Ubuntu",
            "debian" => "Debian",
            "amazonlinux" | "amzn" => "Amazon Linux",
            "rhel" => "Red Hat",
            "centos" => "CentOS",
            other => other,
        };
        let variant = match self.variant.as_str() {
            "desktop" => " Desktop",
            "server" => " Server",
            _ => "",
        };
        format!("{} {}{} ({})", distro, self.version, variant, self.arch)
    }
}

/// Perform the install on a freshly-provisioned tester VM.
///
/// Fast path: downloads pre-built binaries from GitHub releases instead of
/// compiling from source. Typical runtime: 30-60 seconds.
pub async fn install_tester<F>(tester: &TesterTarget, progress: F) -> anyhow::Result<TesterOsInfo>
where
    F: Fn(&str) + Send + Sync,
{
    install_tester_with_options(tester, false, progress).await
}

/// Install with optional Chrome + chrome-harness for browser benchmarks.
pub async fn install_tester_with_options<F>(
    tester: &TesterTarget,
    install_chrome_harness: bool,
    progress: F,
) -> anyhow::Result<TesterOsInfo>
where
    F: Fn(&str) + Send + Sync,
{
    let ip = tester
        .public_ip
        .as_deref()
        .ok_or_else(|| anyhow!("tester has no public_ip; cannot install"))?;
    let user = tester.ssh_user.as_str();

    progress("waiting for SSH");
    wait_for_ssh(ip, user).await.context("SSH readiness")?;

    progress("detecting OS");
    let os_info = detect_os(ip, user).await?;
    tracing::info!(
        tester_id = %tester.tester_id,
        os = %os_info.label(),
        "Detected tester OS"
    );

    if verify_installed(ip, user).await.unwrap_or(false) {
        progress("already installed, skipping");
        return Ok(os_info);
    }

    // Minimal prereqs: just curl + tar. No git, no nodejs, no compilation.
    progress("installing curl + tar");
    install_prereqs(ip, user, &os_info).await?;

    progress("downloading networker-tester binary");
    download_binary(ip, user, "networker-tester", &os_info).await?;

    progress("downloading networker-agent binary");
    download_binary(ip, user, "networker-agent", &os_info).await?;

    progress("installing systemd service");
    install_systemd_service(ip, user).await?;

    // Chrome harness is optional — only for browser benchmarks
    if install_chrome_harness {
        progress("installing Chrome + chrome-harness (optional)");
        install_browser_harness(ip, user).await?;
    }

    progress("verifying install");
    verify_binaries(ip, user).await?;

    progress("install complete");
    tracing::info!(tester_id = %tester.tester_id, "tester install complete");
    Ok(os_info)
}

/// Detect OS distribution, version, variant, arch, kernel via `/etc/os-release` + `uname`.
pub async fn detect_os(ip: &str, user: &str) -> anyhow::Result<TesterOsInfo> {
    let out = ssh_run(
        ip,
        user,
        "cat /etc/os-release 2>/dev/null; echo '---'; uname -m; uname -r; \
         dpkg -l ubuntu-desktop 2>/dev/null | grep -q '^ii' && echo 'VARIANT=desktop' || echo 'VARIANT=server'",
    )
    .await
    .context("detect_os: failed to read /etc/os-release")?;

    let mut distro = String::new();
    let mut version = String::new();
    let mut variant = "server".to_string();
    let mut arch = "x86_64".to_string();
    let mut kernel = String::new();

    let parts: Vec<&str> = out.splitn(2, "---").collect();
    if let Some(os_release) = parts.first() {
        for line in os_release.lines() {
            let line = line.trim();
            if let Some(v) = line.strip_prefix("ID=") {
                distro = v.trim_matches('"').to_lowercase();
            } else if let Some(v) = line.strip_prefix("VERSION_ID=") {
                version = v.trim_matches('"').to_string();
            }
        }
    }
    if let Some(rest) = parts.get(1) {
        let lines: Vec<&str> = rest.lines().filter(|l| !l.trim().is_empty()).collect();
        if let Some(a) = lines.first() {
            arch = a.trim().to_string();
        }
        if let Some(k) = lines.get(1) {
            kernel = k.trim().to_string();
        }
        for l in lines {
            if let Some(v) = l.trim().strip_prefix("VARIANT=") {
                variant = v.to_string();
            }
        }
    }

    Ok(TesterOsInfo {
        distro,
        version,
        variant,
        arch,
        kernel,
    })
}

async fn install_prereqs(ip: &str, user: &str, os: &TesterOsInfo) -> anyhow::Result<()> {
    // Handle apt-get lock contention: if unattended-upgrades is running,
    // wait for it (max 60s) instead of failing immediately.
    let pkg_manager = match os.distro.as_str() {
        "ubuntu" | "debian" => "apt",
        "amzn" | "amazonlinux" | "rhel" | "centos" | "fedora" => "dnf",
        _ => "apt",
    };

    let cmd = match pkg_manager {
        "apt" => {
            "export DEBIAN_FRONTEND=noninteractive; \
             for i in $(seq 1 12); do \
               sudo fuser /var/lib/dpkg/lock-frontend >/dev/null 2>&1 || break; \
               echo 'waiting for apt lock...'; sleep 5; \
             done; \
             sudo apt-get install -y -qq curl tar ca-certificates < /dev/null"
        }
        "dnf" => "sudo dnf install -y curl tar ca-certificates",
        _ => "true",
    };

    ssh_run(ip, user, cmd)
        .await
        .with_context(|| format!("install prereqs via {pkg_manager}"))?;
    Ok(())
}

/// Download a pre-built binary from GitHub releases, extract, install to /usr/local/bin.
async fn download_binary(
    ip: &str,
    user: &str,
    binary: &str,
    os: &TesterOsInfo,
) -> anyhow::Result<()> {
    let tag = release_tag();
    let target = os.release_target();
    let url = format!(
        "https://github.com/irlm/networker-tester/releases/download/{tag}/{binary}-{target}.tar.gz"
    );
    let cmd = format!(
        "set -e; \
         curl -fsSL --retry 3 --retry-delay 2 {url} -o /tmp/{binary}.tar.gz < /dev/null && \
         tar xzf /tmp/{binary}.tar.gz -C /tmp && \
         sudo install -m 0755 /tmp/{binary} /usr/local/bin/{binary} && \
         rm -f /tmp/{binary}.tar.gz /tmp/{binary}"
    );
    ssh_run(ip, user, &cmd)
        .await
        .with_context(|| format!("download {binary} from {url}"))?;
    Ok(())
}

async fn install_systemd_service(ip: &str, user: &str) -> anyhow::Result<()> {
    // Create a systemd user service for networker-agent (runs as regular user)
    let service = "[Unit]
Description=Networker Agent
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/networker-agent
Restart=on-failure
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=default.target
";
    let cmd = format!(
        "mkdir -p ~/.config/systemd/user && \
         cat > ~/.config/systemd/user/networker-agent.service <<'EOF'
{service}
EOF
         systemctl --user daemon-reload && \
         systemctl --user enable networker-agent.service 2>/dev/null || true"
    );
    ssh_run(ip, user, &cmd)
        .await
        .context("install networker-agent systemd service")?;
    Ok(())
}

async fn install_browser_harness(ip: &str, user: &str) -> anyhow::Result<()> {
    let tag = release_tag();
    // Install Chrome + use NodeSource for Node.js 20 (Ubuntu default is too old)
    let cmd = format!(
        "set -e; \
         export DEBIAN_FRONTEND=noninteractive; \
         # Install Chrome \
         command -v google-chrome >/dev/null 2>&1 || (\
           curl -fsSL https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb \
             -o /tmp/chrome.deb < /dev/null && \
           sudo apt-get install -y -qq /tmp/chrome.deb < /dev/null && \
           rm -f /tmp/chrome.deb \
         ); \
         # Install Node.js 20 from NodeSource (Ubuntu's default is 12-18) \
         command -v node >/dev/null 2>&1 || (\
           curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash - < /dev/null && \
           sudo apt-get install -y -qq nodejs < /dev/null \
         ); \
         # Download chrome-harness files from the release \
         sudo mkdir -p /opt/bench/chrome-harness && \
         sudo chown $(whoami):$(whoami) /opt/bench/chrome-harness && \
         curl -fsSL https://github.com/irlm/networker-tester/archive/refs/tags/{tag}.tar.gz \
           -o /tmp/nwk.tar.gz < /dev/null && \
         tar xzf /tmp/nwk.tar.gz -C /tmp && \
         cp /tmp/networker-tester-*/benchmarks/chrome-harness/package.json /opt/bench/chrome-harness/ && \
         cp /tmp/networker-tester-*/benchmarks/chrome-harness/runner.js /opt/bench/chrome-harness/ && \
         cp /tmp/networker-tester-*/benchmarks/chrome-harness/test-page.html /opt/bench/chrome-harness/ && \
         rm -rf /tmp/nwk.tar.gz /tmp/networker-tester-*/ && \
         cd /opt/bench/chrome-harness && npm install --production < /dev/null"
    );
    ssh_run(ip, user, &cmd)
        .await
        .context("install Chrome + chrome-harness")?;
    Ok(())
}

async fn verify_binaries(ip: &str, user: &str) -> anyhow::Result<()> {
    ssh_run(
        ip,
        user,
        "test -x /usr/local/bin/networker-tester && \
         test -x /usr/local/bin/networker-agent && \
         /usr/local/bin/networker-tester --version",
    )
    .await
    .context("verify: networker-tester + networker-agent installed")?;
    Ok(())
}

async fn verify_installed(ip: &str, user: &str) -> anyhow::Result<bool> {
    Ok(ssh_run(
        ip,
        user,
        "test -x /usr/local/bin/networker-tester && test -x /usr/local/bin/networker-agent",
    )
    .await
    .is_ok())
}

/// Poll SSH until the host accepts a trivial command.
async fn wait_for_ssh(ip: &str, user: &str) -> anyhow::Result<()> {
    let total_secs: u32 = std::env::var("DASHBOARD_TESTER_SSH_WAIT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300)
        .clamp(60, 900);
    let attempts = total_secs / 5;
    for attempt in 1..=attempts {
        if ssh_run(ip, user, "true").await.is_ok() {
            return Ok(());
        }
        tracing::debug!(
            attempt,
            ip,
            attempts,
            "SSH not ready (attempt {attempt}/{attempts})"
        );
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
    anyhow::bail!("SSH did not become ready after {total_secs} seconds")
}

async fn ssh_run(ip: &str, user: &str, cmd: &str) -> anyhow::Result<String> {
    let target = format!("{user}@{ip}");
    let output = tokio::process::Command::new("ssh")
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg("-o")
        .arg("ConnectTimeout=10")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg(&target)
        .arg(cmd)
        .output()
        .await
        .with_context(|| format!("ssh spawn failed: {target}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "ssh {target} failed (exit {:?}): {}{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout),
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_target_matches_arch() {
        let os = TesterOsInfo {
            distro: "ubuntu".into(),
            version: "24.04".into(),
            variant: "server".into(),
            arch: "x86_64".into(),
            kernel: "6.8.0".into(),
        };
        assert_eq!(os.release_target(), "x86_64-unknown-linux-musl");

        let os_arm = TesterOsInfo {
            arch: "aarch64".into(),
            ..os
        };
        assert_eq!(os_arm.release_target(), "aarch64-unknown-linux-musl");
    }

    #[test]
    fn label_includes_distro_version_variant_arch() {
        let os = TesterOsInfo {
            distro: "ubuntu".into(),
            version: "24.04".into(),
            variant: "server".into(),
            arch: "x86_64".into(),
            kernel: "6.8.0".into(),
        };
        assert_eq!(os.label(), "Ubuntu 24.04 Server (x86_64)");

        let desk = TesterOsInfo {
            variant: "desktop".into(),
            ..os
        };
        assert_eq!(desk.label(), "Ubuntu 24.04 Desktop (x86_64)");
    }
}
