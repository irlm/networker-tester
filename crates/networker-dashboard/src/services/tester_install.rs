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
///
/// RR-009: if the tester already passes `verify_installed` (Chrome on
/// PATH + chrome-harness runner present), the function short-circuits
/// with a `"already installed, skipping"` progress log. This makes
/// `POST /testers/{tid}/upgrade` idempotent on already-provisioned
/// testers and also turns recoveries of fresh-install failures into
/// a fast no-op when they got far enough. Fresh installs are unaffected
/// because `verify_installed` returns false on a bare Azure VM.
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

    if verify_installed(ip, user).await.unwrap_or(false) {
        progress("already installed, skipping");
        tracing::info!(
            tester_id = %tester.tester_id,
            "tester install short-circuited: already installed"
        );
        return Ok(());
    }

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
    // RR-008: pin to the dashboard's own release tag so newly-created
    // testers can't inherit a broken tip-of-main. For release builds
    // (e.g. CARGO_PKG_VERSION=0.25.0) the tag "v0.25.0" exists and will
    // be used. For unreleased main builds the tag doesn't exist yet and
    // the clone will fail loudly — strictly better than silently cloning
    // a potentially-broken main branch.
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    let clone_cmd = format!(
        "rm -rf /tmp/nwk-repo && \
         git clone --depth 1 --branch {tag} \
           https://github.com/irlm/networker-tester.git /tmp/nwk-repo < /dev/null"
    );
    ssh_run(ip, user, &clone_cmd)
        .await
        .with_context(|| format!("git clone networker-tester at {tag}"))?;

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
    // RR-009: no more `2>/dev/null` on dpkg — silencing stderr meant a
    // failed Chrome install was invisible, and the downstream `command -v
    // google-chrome` verification passed anyway on a broken tester.
    // If dpkg fails, `apt-get install -f` resolves broken dependencies
    // and retries; if that still fails, `set -e` makes the whole step
    // exit non-zero and surfaces dpkg's stderr through ssh_run.
    ssh_run(
        ip,
        user,
        "set -e; \
         command -v google-chrome >/dev/null 2>&1 || { \
           export DEBIAN_FRONTEND=noninteractive && \
           sudo dpkg -i /tmp/chrome.deb || \
             (sudo apt-get install -y -qq -f < /dev/null && \
              sudo dpkg -i /tmp/chrome.deb); \
           sudo apt-get install -y -qq nodejs npm < /dev/null; \
           rm -f /tmp/chrome.deb; }; \
         command -v npm >/dev/null 2>&1 || \
           sudo apt-get install -y -qq nodejs npm < /dev/null",
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

/// Return true if Chrome is on PATH and the chrome-harness runner exists.
/// Used by `install_tester` in upgrade mode to short-circuit on an
/// already-provisioned tester.
async fn verify_installed(ip: &str, user: &str) -> anyhow::Result<bool> {
    match ssh_run(
        ip,
        user,
        "command -v google-chrome >/dev/null 2>&1 && \
         test -f /opt/bench/chrome-harness/runner.js",
    )
    .await
    {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// Poll SSH until the host accepts a trivial command.
///
/// RR-012: Azure VMs with `unattended-upgrades` + cloud-init on first
/// boot routinely need 6-10 minutes. Default window is 10 minutes
/// (60 attempts × 10s). Operators can override via the env var
/// `DASHBOARD_TESTER_SSH_WAIT_SECS` (total seconds, clamped to [60, 900]).
async fn wait_for_ssh(ip: &str, user: &str) -> anyhow::Result<()> {
    let total_secs: u32 = std::env::var("DASHBOARD_TESTER_SSH_WAIT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600)
        .clamp(60, 900);
    let attempts = total_secs / 10;
    for attempt in 1..=attempts {
        if ssh_run(ip, user, "true").await.is_ok() {
            // RR-016: record the host key on first success so subsequent
            // commands verify (accept-new also does this automatically via
            // UserKnownHostsFile=~/.ssh/known_hosts, but we force-refresh
            // here to avoid relying on per-invocation side effects).
            return Ok(());
        }
        tracing::debug!(
            attempt,
            ip,
            attempts,
            "SSH not ready (attempt {attempt}/{attempts}) — sleeping 10s"
        );
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }
    anyhow::bail!("SSH did not become ready after {total_secs} seconds")
}

/// Run a single remote command over SSH, capturing stdout on success.
///
/// Uses `tokio::process::Command`, which `execvp`s `ssh` directly (no
/// intermediate shell on the dashboard host), so arguments are passed
/// as a flat argv with no local shell interpolation.
async fn ssh_run(ip: &str, user: &str, cmd: &str) -> anyhow::Result<String> {
    let target = format!("{user}@{ip}");
    // RR-016: `accept-new` is the materially-safer default — it auto-
    // records the host key on first successful connect and verifies on
    // subsequent connects, detecting MITM / key rotation. The previous
    // `StrictHostKeyChecking=no` + `UserKnownHostsFile=/dev/null` config
    // disabled verification entirely.
    //
    // TODO(follow-up): persist per-tester known_hosts under
    // ~/.ssh/known_hosts_testers/{tester_id} so rotating the dashboard's
    // global known_hosts doesn't silently re-accept rotated keys.
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
