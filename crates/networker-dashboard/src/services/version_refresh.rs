//! Latest-version refresh — periodically polls GitHub releases for the
//! newest tag and caches it in an Arc<RwLock<String>> for REST handlers.

#![allow(dead_code)] // wired in Task 17 + 34

use rand::RngExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

const REFRESH_BASE: Duration = Duration::from_secs(6 * 3600);
const REFRESH_JITTER_MAX_SECS: u64 = 30 * 60;
const GITHUB_LATEST: &str = "https://api.github.com/repos/irlm/networker-tester/releases/latest";

/// Error returned by `fetch_github_latest` when GitHub responds with a
/// rate-limit status (403 or 429). Used by the loop to silently fall
/// back to the cache floor instead of logging a warning (RR-011).
#[derive(Debug)]
struct RateLimited;

impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "github rate limited")
    }
}

impl std::error::Error for RateLimited {}

pub async fn refresh_latest_version_loop(cache: Arc<RwLock<String>>) {
    loop {
        // RR-011: each sleep is base + rand(0..30min) so multi-dashboard
        // deploys don't all hit api.github.com at the same 6h mark.
        let jitter_secs = rand::rng().random_range(0..=REFRESH_JITTER_MAX_SECS);
        let sleep = REFRESH_BASE + Duration::from_secs(jitter_secs);
        tokio::time::sleep(sleep).await;
        match refresh_now(cache.clone()).await {
            Ok(v) => tracing::info!(version = %v, "latest-version refresh succeeded"),
            Err(e) => {
                // Rate-limit responses fall back to the cache floor
                // silently (debug level) — we expect these when a shared
                // NAT exhausts the unauthenticated GitHub quota.
                if e.downcast_ref::<RateLimited>().is_some() {
                    tracing::debug!(
                        "latest-version refresh rate limited; using CARGO_PKG_VERSION floor"
                    );
                } else {
                    tracing::warn!(error = ?e, "latest-version refresh failed");
                }
            }
        }
    }
}

pub async fn refresh_now(cache: Arc<RwLock<String>>) -> anyhow::Result<String> {
    let floor = env!("CARGO_PKG_VERSION");
    let resolved = match fetch_github_latest().await {
        Ok(remote) => pick_higher_semver(floor, Some(&remote)),
        Err(e) => {
            tracing::debug!(error = ?e, "github fetch failed, falling back to CARGO_PKG_VERSION");
            floor.to_string()
        }
    };
    *cache.write().await = resolved.clone();
    Ok(resolved)
}

async fn fetch_github_latest() -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .user_agent("networker-dashboard-version-refresh")
        .timeout(Duration::from_secs(15))
        .build()?;
    let resp = client.get(GITHUB_LATEST).send().await?;
    let status = resp.status();
    // RR-011: surface rate limits via a typed error so the loop can
    // downgrade the log level.
    if status.as_u16() == 403 || status.as_u16() == 429 {
        return Err(anyhow::Error::new(RateLimited));
    }
    if !status.is_success() {
        anyhow::bail!("github latest returned {}", status);
    }
    let body: serde_json::Value = resp.json().await?;
    let tag = body
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("github response missing tag_name"))?;
    // Strip leading 'v' if present — GitHub tags are usually 'v0.25.0'.
    Ok(tag.trim_start_matches('v').to_string())
}

/// Return whichever of `a` and `b` (if present) is the higher semver. Invalid
/// versions on either side cause the OTHER to win; if both are invalid, `a`.
pub fn pick_higher_semver(a: &str, b: Option<&str>) -> String {
    let Some(b) = b else {
        return a.to_string();
    };
    match (parse_semver(a), parse_semver(b)) {
        (Some(pa), Some(pb)) => {
            if pb > pa {
                b.to_string()
            } else {
                a.to_string()
            }
        }
        (Some(_), None) => a.to_string(),
        (None, Some(_)) => b.to_string(),
        (None, None) => a.to_string(),
    }
}

fn parse_semver(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.trim_start_matches('v').split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    // Allow pre-release/metadata suffix on patch: "0.25.0-rc.1" → patch=0
    let patch = parts
        .next()?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse::<u32>()
        .ok()?;
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_higher_semver_returns_newer() {
        assert_eq!(pick_higher_semver("0.24.0", Some("0.25.1")), "0.25.1");
    }

    #[test]
    fn pick_higher_semver_floor_wins_when_remote_older() {
        assert_eq!(pick_higher_semver("0.25.0", Some("0.24.9")), "0.25.0");
    }

    #[test]
    fn pick_higher_semver_no_remote_returns_floor() {
        assert_eq!(pick_higher_semver("0.25.0", None), "0.25.0");
    }

    #[test]
    fn pick_higher_semver_strips_v_prefix() {
        assert_eq!(pick_higher_semver("0.24.0", Some("v0.25.0")), "v0.25.0");
    }

    #[test]
    fn parse_semver_handles_prerelease() {
        assert_eq!(parse_semver("0.25.0-rc.1"), Some((0, 25, 0)));
    }
}
