//! One-time tester install: Chrome + Node.js + chrome-harness files.
//!
//! Called from the tester-create background task (Task 15) after the
//! Azure VM is provisioned. Previously run per benchmark via the
//! orchestrator's `deploy_chrome_harness` (Task 24 deletes that copy).
//!
//! The function is idempotent per step: Chrome is skipped if already on
//! PATH, the repo clone is fresh, and `npm install` is re-run from the
//! deployed directory. All commands run over SSH using the ambient SSH
//! identity (agent / default key) — per-tester keys are a future task.

#![allow(dead_code)] // wired in Task 15

use anyhow::{anyhow, Context};
use uuid::Uuid;

/// Minimal view of a persistent tester needed to run the install.
///
/// This intentionally does not depend on `crate::db::project_testers`
/// because the `db` module lives in the binary tree (not the lib
/// crate). Task 15 constructs this from a `ProjectTesterRow` at the
/// call site.
#[derive(Debug, Clone)]
pub struct TesterTarget {
    pub tester_id: Uuid,
    pub public_ip: Option<String>,
    pub ssh_user: String,
}

/// Perform the one-time install on a freshly-provisioned tester VM.
///
/// `progress` is invoked with a short human-readable label before each
/// step so the caller can surface it via `project_tester.status_message`.
pub async fn install_tester<F>(tester: &TesterTarget, progress: F) -> anyhow::Result<()>
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

    progress("stopping unattended-upgrades");
    ssh_run(
        ip,
        user,
        "sudo systemctl stop unattended-upgrades 2>/dev/null || true",
    )
    .await?;

    progress("installing apt prerequisites");
    ssh_run(
        ip,
        user,
        "export DEBIAN_FRONTEND=noninteractive && \
         sudo apt-get update -qq < /dev/null && \
         sudo apt-get install -y -qq curl git ca-certificates < /dev/null",
    )
    .await
    .context("apt-get install curl/git/ca-certificates")?;

    progress("cloning networker-tester repo");
    ssh_run(
        ip,
        user,
        "rm -rf /tmp/nwk-repo && \
         git clone https://github.com/irlm/networker-tester.git /tmp/nwk-repo < /dev/null",
    )
    .await
    .context("git clone networker-tester")?;

    progress("creating /opt/bench/chrome-harness");
    ssh_run(
        ip,
        user,
        "sudo mkdir -p /opt/bench/chrome-harness && \
         sudo chown $(whoami):$(whoami) /opt/bench/chrome-harness",
    )
    .await?;

    progress("downloading Chrome .deb");
    ssh_run(
        ip,
        user,
        "command -v google-chrome >/dev/null 2>&1 || \
         curl -fsSL https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb \
           -o /tmp/chrome.deb < /dev/null",
    )
    .await?;

    progress("installing Chrome + Node.js + npm");
    ssh_run(
        ip,
        user,
        "command -v google-chrome >/dev/null 2>&1 || { \
           export DEBIAN_FRONTEND=noninteractive && \
           sudo dpkg -i /tmp/chrome.deb 2>/dev/null; \
           sudo apt-get install -y -qq -f < /dev/null && \
           sudo apt-get install -y -qq nodejs npm < /dev/null && \
           rm -f /tmp/chrome.deb; }; \
         command -v npm >/dev/null 2>&1 || sudo apt-get install -y -qq nodejs npm < /dev/null",
    )
    .await
    .context("chrome + node install")?;

    progress("deploying chrome-harness files");
    ssh_run(
        ip,
        user,
        "cp /tmp/nwk-repo/benchmarks/chrome-harness/package.json /opt/bench/chrome-harness/ && \
         cp /tmp/nwk-repo/benchmarks/chrome-harness/runner.js /opt/bench/chrome-harness/ && \
         cp /tmp/nwk-repo/benchmarks/chrome-harness/test-page.html /opt/bench/chrome-harness/ && \
         cd /opt/bench/chrome-harness && npm install --production --silent < /dev/null",
    )
    .await
    .context("deploy chrome-harness files + npm install")?;

    progress("verifying install");
    ssh_run(
        ip,
        user,
        "test -f /opt/bench/chrome-harness/runner.js && \
         command -v google-chrome >/dev/null 2>&1",
    )
    .await
    .context("verification: runner.js + google-chrome")?;

    progress("install complete");
    tracing::info!(tester_id = %tester.tester_id, "tester install complete");
    Ok(())
}

/// Poll SSH until the host accepts a trivial command, up to 5 minutes.
async fn wait_for_ssh(ip: &str, user: &str) -> anyhow::Result<()> {
    for attempt in 1..=30u32 {
        if ssh_run(ip, user, "true").await.is_ok() {
            return Ok(());
        }
        tracing::debug!(
            attempt,
            ip,
            "SSH not ready (attempt {attempt}/30) — sleeping 10s"
        );
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }
    anyhow::bail!("SSH did not become ready after 5 minutes")
}

/// Run a single remote command over SSH, capturing stdout on success.
///
/// Uses `tokio::process::Command`, which `execvp`s `ssh` directly (no
/// intermediate shell on the dashboard host), so arguments are passed
/// as a flat argv with no local shell interpolation.
async fn ssh_run(ip: &str, user: &str, cmd: &str) -> anyhow::Result<String> {
    let target = format!("{user}@{ip}");
    let output = tokio::process::Command::new("ssh")
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg("-o")
        .arg("UserKnownHostsFile=/dev/null")
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
