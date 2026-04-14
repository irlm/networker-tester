//! Pre-flight checks for tester VM creation.
//!
//! Runs before `POST /testers` to verify that the cloud account has
//! capacity + permissions, and auto-resolves what's safe (e.g. cleaning
//! unattached Azure Public IPs).

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::auth::{AuthUser, ProjectContext, ProjectRole};
use crate::AppState;
use networker_dashboard::services::cloud_provider::{
    AwsProvider, AzureProvider, CloudProvider, GcpProvider,
};

#[derive(Debug, Deserialize)]
pub struct PrecheckRequest {
    pub cloud: String,
    pub region: String,
    pub requested_os: Option<String>,
    pub requested_variant: Option<String>,
}

#[derive(Debug, Serialize, Default)]
pub struct PrecheckResponse {
    /// "ok" | "warning" | "blocked"
    pub status: String,
    /// Issues that are fully blocking — must be resolved before create will succeed.
    pub blockers: Vec<PrecheckIssue>,
    /// Warnings — create may still work but user should be aware.
    pub warnings: Vec<PrecheckIssue>,
    /// What the precheck auto-resolved (e.g. deleted orphan IPs).
    pub auto_resolved: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct PrecheckIssue {
    /// Short code (stable identifier)
    pub code: String,
    /// Human-readable message
    pub message: String,
    /// What the user can do to fix it
    pub resolution: String,
}

/// `POST /api/projects/{pid}/testers/precheck` — run pre-flight checks.
async fn precheck(
    State(state): State<Arc<AppState>>,
    Path(_project_id): Path<String>,
    req: axum::extract::Request,
) -> Result<Json<PrecheckResponse>, (StatusCode, String)> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let _user = req.extensions().get::<AuthUser>().unwrap().clone();
    crate::auth::require_project_role(&ctx, ProjectRole::Operator)
        .map_err(|s| (s, "Operator role required".into()))?;

    let body = axum::body::to_bytes(req.into_body(), 1024 * 16)
        .await
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid request body".into()))?;
    let req: PrecheckRequest =
        serde_json::from_slice(&body).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    let mut resp = PrecheckResponse {
        status: "ok".to_string(),
        ..Default::default()
    };

    // Load the cloud_account for this cloud
    let client = state
        .db
        .get()
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB pool error".into()))?;
    let acct_row = client
        .query_opt(
            "SELECT credentials_enc, credentials_nonce FROM cloud_account \
             WHERE project_id = $1 AND provider = $2 AND status = 'active' \
             ORDER BY created_at ASC LIMIT 1",
            &[&ctx.project_id, &req.cloud],
        )
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "DB query error".into()))?;

    let row = match acct_row {
        Some(r) => r,
        None => {
            resp.blockers.push(PrecheckIssue {
                code: "no_cloud_account".into(),
                message: format!(
                    "No active {} cloud account found for this project",
                    req.cloud
                ),
                resolution: format!(
                    "Go to Settings → Cloud → Add Account, select {} and provide credentials",
                    req.cloud
                ),
            });
            resp.status = "blocked".into();
            return Ok(Json(resp));
        }
    };

    // Decrypt credentials
    let cred_key = match state.credential_key.as_ref() {
        Some(k) => k,
        None => {
            resp.blockers.push(PrecheckIssue {
                code: "no_credential_key".into(),
                message: "Server cannot decrypt cloud credentials".into(),
                resolution: "Admin must set DASHBOARD_CREDENTIAL_KEY on the server".into(),
            });
            resp.status = "blocked".into();
            return Ok(Json(resp));
        }
    };
    let enc: Vec<u8> = row.get("credentials_enc");
    let nonce_bytes: Vec<u8> = row.get("credentials_nonce");
    let nonce: [u8; 12] = match nonce_bytes.as_slice().try_into() {
        Ok(n) => n,
        Err(_) => {
            resp.blockers.push(PrecheckIssue {
                code: "invalid_nonce".into(),
                message: "Stored credentials are corrupted".into(),
                resolution: "Delete and recreate the cloud account".into(),
            });
            resp.status = "blocked".into();
            return Ok(Json(resp));
        }
    };
    let creds = match crate::crypto::decrypt_with_fallback(
        &enc,
        &nonce,
        cred_key,
        state.credential_key_old.as_ref(),
    ) {
        Ok(pt) => serde_json::from_slice::<serde_json::Value>(&pt).unwrap_or(serde_json::json!({})),
        Err(_) => {
            resp.blockers.push(PrecheckIssue {
                code: "decrypt_failed".into(),
                message: "Cloud credentials could not be decrypted".into(),
                resolution: "Delete and recreate the cloud account with current credentials".into(),
            });
            resp.status = "blocked".into();
            return Ok(Json(resp));
        }
    };

    // Run provider-specific precheck
    match req.cloud.as_str() {
        "azure" => precheck_azure(&creds, &req, &mut resp).await,
        "aws" => precheck_aws(&creds, &req, &mut resp).await,
        "gcp" => precheck_gcp(&creds, &req, &mut resp).await,
        other => {
            resp.blockers.push(PrecheckIssue {
                code: "unknown_cloud".into(),
                message: format!("Unknown cloud provider: {}", other),
                resolution: "Use azure, aws, or gcp".into(),
            });
            resp.status = "blocked".into();
        }
    }

    // Final status
    if !resp.blockers.is_empty() {
        resp.status = "blocked".into();
    } else if !resp.warnings.is_empty() {
        resp.status = "warning".into();
    }

    Ok(Json(resp))
}

async fn precheck_azure(
    creds: &serde_json::Value,
    req: &PrecheckRequest,
    resp: &mut PrecheckResponse,
) {
    // Build Azure provider config from creds
    let config = serde_json::json!({
        "subscription_id": creds.get("subscription_id").and_then(|v| v.as_str()).unwrap_or(""),
        "resource_group": creds.get("resource_group").and_then(|v| v.as_str()).unwrap_or("networker-testers"),
        "tenant_id": creds.get("tenant_id").and_then(|v| v.as_str()).unwrap_or(""),
        "client_id": creds.get("client_id").and_then(|v| v.as_str()).unwrap_or(""),
        "client_secret": creds.get("client_secret").and_then(|v| v.as_str()).unwrap_or(""),
        "identity_type": "service_principal",
    });

    let provider = match AzureProvider::from_config(&config) {
        Ok(p) => CloudProvider::Azure(p),
        Err(e) => {
            resp.blockers.push(PrecheckIssue {
                code: "azure_config_invalid".into(),
                message: format!("Azure cloud account config invalid: {e}"),
                resolution: "Edit the cloud account and fill in all required fields".into(),
            });
            return;
        }
    };

    // Check Public IP quota + auto-delete orphans
    let subscription = config["subscription_id"].as_str().unwrap_or("");
    match azure_list_orphan_ips(&provider, subscription, &req.region).await {
        Ok(orphans) if !orphans.is_empty() => {
            let n = orphans.len();
            let deleted = azure_delete_ips(&provider, subscription, &orphans).await;
            if deleted > 0 {
                resp.auto_resolved.push(format!(
                    "Deleted {deleted} unattached Azure Public IP(s) (out of {n} orphans found) to free up subscription quota",
                ));
            }
        }
        Err(e) => {
            resp.warnings.push(PrecheckIssue {
                code: "azure_ip_list_failed".into(),
                message: format!("Could not list Azure Public IPs: {e}"),
                resolution: "Manually review orphan resources in portal.azure.com".into(),
            });
        }
        _ => {}
    }

    // Check region supports the requested OS (e.g. Ubuntu Desktop may not be in all regions)
    if req.requested_os.as_deref() == Some("windows-11")
        && req.requested_variant.as_deref() == Some("desktop")
    {
        resp.warnings.push(PrecheckIssue {
            code: "azure_windows_11_license".into(),
            message: "Windows 11 Desktop images require Multi-Tenant Hosting Rights or Visual Studio license".into(),
            resolution: "Check Azure Marketplace licensing for Windows 11 Desktop in your subscription".into(),
        });
    }
}

async fn precheck_aws(
    creds: &serde_json::Value,
    req: &PrecheckRequest,
    resp: &mut PrecheckResponse,
) {
    let mut config = creds.clone();
    if let Some(obj) = config.as_object_mut() {
        obj.insert(
            "region".to_string(),
            serde_json::Value::String(req.region.clone()),
        );
    }

    let _ = match AwsProvider::from_config(&config) {
        Ok(p) => p,
        Err(e) => {
            resp.blockers.push(PrecheckIssue {
                code: "aws_config_invalid".into(),
                message: format!("AWS config invalid: {e}"),
                resolution:
                    "Edit the AWS cloud account and provide access_key_id + secret_access_key"
                        .into(),
            });
            return;
        }
    };

    // Ping STS to verify credentials haven't expired
    let access_key = creds
        .get("access_key_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let secret = creds
        .get("secret_access_key")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let session_token = creds
        .get("session_token")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let mut cmd = tokio::process::Command::new("aws");
    cmd.arg("sts")
        .arg("get-caller-identity")
        .env("AWS_ACCESS_KEY_ID", access_key)
        .env("AWS_SECRET_ACCESS_KEY", secret);
    if !session_token.is_empty() {
        cmd.env("AWS_SESSION_TOKEN", session_token);
    }
    match cmd.output().await {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            if stderr.contains("ExpiredToken") || stderr.contains("RequestExpired") {
                resp.blockers.push(PrecheckIssue {
                    code: "aws_credentials_expired".into(),
                    message: "AWS session token has expired".into(),
                    resolution: "Run `aws sso login`, then `aws configure export-credentials --format env` and update the cloud account".into(),
                });
            } else if stderr.contains("InvalidClientTokenId") {
                resp.blockers.push(PrecheckIssue {
                    code: "aws_invalid_credentials".into(),
                    message: "AWS access key is invalid".into(),
                    resolution: "Check that access_key_id is correct (starts with AKIA or ASIA)"
                        .into(),
                });
            } else {
                resp.blockers.push(PrecheckIssue {
                    code: "aws_sts_failed".into(),
                    message: format!("AWS STS validation failed: {}", stderr.trim()),
                    resolution: "Verify AWS credentials in cloud account".into(),
                });
            }
        }
        Err(e) => {
            resp.blockers.push(PrecheckIssue {
                code: "aws_cli_missing".into(),
                message: format!("Cannot run aws CLI: {e}"),
                resolution: "Install AWS CLI on the dashboard server".into(),
            });
        }
    }
}

async fn precheck_gcp(
    creds: &serde_json::Value,
    _req: &PrecheckRequest,
    resp: &mut PrecheckResponse,
) {
    let config = creds.clone();
    if GcpProvider::from_config(&config).is_err() {
        resp.blockers.push(PrecheckIssue {
            code: "gcp_config_invalid".into(),
            message: "GCP cloud account config invalid".into(),
            resolution: "Edit the GCP cloud account and provide a valid service account JSON key"
                .into(),
        });
        return;
    }

    // Check local SSH public key exists (needed to inject into GCP VMs)
    let home = std::env::var("HOME").unwrap_or_default();
    let pub_key_path = format!("{home}/.ssh/id_rsa.pub");
    if !std::path::Path::new(&pub_key_path).exists() {
        resp.warnings.push(PrecheckIssue {
            code: "gcp_no_local_ssh_key".into(),
            message: "Dashboard host has no ~/.ssh/id_rsa.pub — GCP VMs will not be reachable via SSH".into(),
            resolution: "Run `ssh-keygen -t rsa -b 4096` on the dashboard host, or GCP will fall back to OS Login (slower)".into(),
        });
    }
}

// ── Azure IP helpers ──────────────────────────────────────────────────────

async fn azure_list_orphan_ips(
    _provider: &CloudProvider,
    subscription: &str,
    _region: &str,
) -> anyhow::Result<Vec<String>> {
    // List all public IPs not currently attached to any NIC
    let output = tokio::process::Command::new("az")
        .arg("network")
        .arg("public-ip")
        .arg("list")
        .arg("--subscription")
        .arg(subscription)
        .arg("--query")
        .arg("[?ipConfiguration==null].id")
        .arg("--output")
        .arg("tsv")
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "az network public-ip list: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let ids: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Ok(ids)
}

async fn azure_delete_ips(_provider: &CloudProvider, subscription: &str, ids: &[String]) -> usize {
    let mut count = 0;
    for id in ids {
        let output = tokio::process::Command::new("az")
            .arg("network")
            .arg("public-ip")
            .arg("delete")
            .arg("--subscription")
            .arg(subscription)
            .arg("--ids")
            .arg(id)
            .output()
            .await;
        if output.map(|o| o.status.success()).unwrap_or(false) {
            count += 1;
        }
    }
    count
}

pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/testers/precheck", post(precheck))
        .with_state(state)
}
