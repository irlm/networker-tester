use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;

#[derive(Serialize)]
pub struct VersionInfo {
    pub dashboard_version: String,
    pub tester_version: Option<String>,
    pub latest_release: Option<String>,
    pub update_available: bool,
    pub endpoints: Vec<EndpointVersion>,
}

#[derive(Serialize)]
pub struct EndpointVersion {
    pub host: String,
    pub version: Option<String>,
    pub reachable: bool,
}

async fn check_versions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<VersionInfo>, StatusCode> {
    let dashboard_version = env!("CARGO_PKG_VERSION").to_string();

    // Get local tester version
    let tester_version = get_tester_version().await;

    // Get latest release from GitHub
    let latest_release = get_latest_release().await;

    // Check if update is available
    let update_available = match (&tester_version, &latest_release) {
        (Some(local), Some(remote)) => {
            let local_clean = local.trim_start_matches('v');
            let remote_clean = remote.trim_start_matches('v');
            version_newer(remote_clean, local_clean)
        }
        _ => false,
    };

    // Check deployed endpoint versions (concurrent, not sequential)
    let mut endpoints = Vec::new();
    if let Ok(client) = state.db.get().await {
        if let Ok(deployments) = crate::db::deployments::list_all(&client, 20, 0).await {
            let mut hosts = Vec::new();
            for dep in &deployments {
                if dep.status != "completed" {
                    continue;
                }
                let ips: Vec<String> = dep
                    .endpoint_ips
                    .as_ref()
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                hosts.extend(ips);
            }
            // Deduplicate and check all hosts concurrently
            hosts.sort();
            hosts.dedup();
            let checks: Vec<_> = hosts
                .iter()
                .map(|host| {
                    let h = host.clone();
                    async move {
                        let (reachable, version) = check_endpoint_version(&h).await;
                        EndpointVersion {
                            host: h,
                            version,
                            reachable,
                        }
                    }
                })
                .collect();
            endpoints = futures::future::join_all(checks).await;
        }
    }

    Ok(Json(VersionInfo {
        dashboard_version,
        tester_version,
        latest_release,
        update_available,
        endpoints,
    }))
}

async fn get_tester_version() -> Option<String> {
    let bin = crate::deploy::agent_provisioner::find_tester_binary_path().await?;
    let output = tokio::process::Command::new(&bin)
        .arg("--version")
        .output()
        .await
        .ok()?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Parse "networker-tester 0.13.19"
        stdout
            .split_whitespace()
            .last()
            .map(|s| s.trim().to_string())
    } else {
        None
    }
}

async fn get_latest_release() -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;
    let resp = client
        .get("https://api.github.com/repos/irlm/networker-tester/releases/latest")
        .header("User-Agent", "networker-dashboard")
        .send()
        .await
        .ok()?;
    let body: serde_json::Value = resp.json().await.ok()?;
    body.get("tag_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim_start_matches('v').to_string())
}

async fn check_endpoint_version(host: &str) -> (bool, Option<String>) {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    // Try HTTPS first, then HTTP
    for url in &[
        format!("https://{host}:8443/health"),
        format!("http://{host}:8080/health"),
    ] {
        if let Ok(resp) = client.get(url).send().await {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                let version = body
                    .get("version")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                return (true, version);
            }
        }
    }
    (false, None)
}

/// Simple semver comparison: returns true if `a` is newer than `b`.
fn version_newer(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> { s.split('.').filter_map(|p| p.parse().ok()).collect() };
    let va = parse(a);
    let vb = parse(b);
    for i in 0..va.len().max(vb.len()) {
        let pa = va.get(i).copied().unwrap_or(0);
        let pb = vb.get(i).copied().unwrap_or(0);
        if pa > pb {
            return true;
        }
        if pa < pb {
            return false;
        }
    }
    false
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/version", get(check_versions))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::version_newer;

    #[test]
    fn newer_patch_detected() {
        assert!(version_newer("0.13.37", "0.13.36"));
    }

    #[test]
    fn same_version_not_newer() {
        assert!(!version_newer("0.13.36", "0.13.36"));
    }

    #[test]
    fn older_patch_not_newer() {
        assert!(!version_newer("0.13.35", "0.13.36"));
    }

    #[test]
    fn newer_minor_detected() {
        assert!(version_newer("0.14.0", "0.13.99"));
    }

    #[test]
    fn newer_major_detected() {
        assert!(version_newer("1.0.0", "0.99.99"));
    }

    #[test]
    fn two_part_version_compared_correctly() {
        assert!(version_newer("1.1", "1.0"));
        assert!(!version_newer("1.0", "1.1"));
    }

    #[test]
    fn missing_patch_treated_as_zero() {
        // "1.0" vs "1.0.0" should be equal
        assert!(!version_newer("1.0", "1.0.0"));
        assert!(!version_newer("1.0.0", "1.0"));
    }

    #[test]
    fn empty_string_handled_safely() {
        assert!(!version_newer("", ""));
        assert!(!version_newer("", "1.0.0"));
        // "" parses to no segments, which is less than "1.0.0"
        assert!(version_newer("1.0.0", ""));
    }

    #[test]
    fn non_numeric_segments_ignored() {
        // parse() on "beta" returns None, filter_map skips it
        assert!(!version_newer("1.0.beta", "1.0.1"));
    }

    #[test]
    fn v_prefix_already_stripped_by_caller() {
        // version_newer is called after trim_start_matches('v')
        // But if not stripped, the 'v' prefix makes the parse fail gracefully
        assert!(!version_newer("v1.0.0", "0.9.0"));
    }
}
