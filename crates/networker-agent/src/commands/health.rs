//! `health` verb: reports agent version, OS/arch, uptime, and (best-effort)
//! free disk space.
//!
//! Intentionally pure-Rust and dependency-free. Uptime is read from
//! `/proc/uptime` on Linux and returns 0 on other platforms. Disk-free is
//! stubbed to `None` for now — a proper cross-platform implementation will
//! land when we're willing to pull in `nix`/`sysinfo`.

use anyhow::Result;
use networker_common::messages::AgentCommandLog;
use serde_json::json;
use tokio::sync::mpsc;

/// Run the `health` verb. Ignores `args` — health takes no parameters.
pub async fn run(
    _args: serde_json::Value,
    log_tx: &mpsc::Sender<AgentCommandLog>,
) -> Result<serde_json::Value> {
    // Health is fast and has nothing interesting to log. The channel is
    // kept in the signature so the dispatcher always passes one and so
    // future verbs can stream logs without changing the dispatcher.
    let _ = log_tx;

    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "uptime_secs": uptime_secs(),
        "disk_free_mb": disk_free_mb("/"),
    }))
}

/// Best-effort system uptime in seconds. Returns 0 on non-Linux or on
/// parse failure.
fn uptime_secs() -> u64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_string))
        .and_then(|s| s.parse::<f64>().ok())
        .map(|f| f as u64)
        .unwrap_or(0)
}

/// Best-effort free-disk reporting. Currently always returns `None` — we
/// don't want to pull in a new dep just for this. A later task can wire
/// in `nix::sys::statvfs` or `sysinfo` once we're ready.
fn disk_free_mb(_path: &str) -> Option<u64> {
    None
}
