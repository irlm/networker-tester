use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;

#[derive(Serialize)]
pub struct ProviderStatus {
    pub available: bool,
    pub authenticated: bool,
    pub account: Option<String>,
}

#[derive(Serialize)]
pub struct CloudStatus {
    pub azure: ProviderStatus,
    pub aws: ProviderStatus,
    pub gcp: ProviderStatus,
    pub ssh: ProviderStatus,
}

async fn check_command(cmd: &str, args: &[&str]) -> (bool, Option<String>) {
    match tokio::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (
                true,
                if stdout.is_empty() {
                    None
                } else {
                    Some(stdout)
                },
            )
        }
        Ok(_) => (false, None),
        Err(_) => (false, None),
    }
}

async fn command_exists(cmd: &str) -> bool {
    tokio::process::Command::new("which")
        .arg(cmd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn check_azure() -> ProviderStatus {
    let available = command_exists("az").await;
    if !available {
        return ProviderStatus {
            available: false,
            authenticated: false,
            account: None,
        };
    }

    let (authenticated, output) =
        check_command("az", &["account", "show", "--query", "name", "-o", "tsv"]).await;
    ProviderStatus {
        available: true,
        authenticated,
        account: output,
    }
}

async fn check_aws() -> ProviderStatus {
    let available = command_exists("aws").await;
    if !available {
        return ProviderStatus {
            available: false,
            authenticated: false,
            account: None,
        };
    }

    let (authenticated, output) = check_command(
        "aws",
        &[
            "sts",
            "get-caller-identity",
            "--query",
            "Account",
            "--output",
            "text",
        ],
    )
    .await;
    ProviderStatus {
        available: true,
        authenticated,
        account: output,
    }
}

async fn check_gcp() -> ProviderStatus {
    let available = command_exists("gcloud").await;
    if !available {
        return ProviderStatus {
            available: false,
            authenticated: false,
            account: None,
        };
    }

    let (authenticated, output) =
        check_command("gcloud", &["config", "get-value", "project"]).await;
    ProviderStatus {
        available: true,
        authenticated: authenticated && output.is_some(),
        account: output,
    }
}

async fn check_ssh() -> ProviderStatus {
    let available = command_exists("ssh").await;
    ProviderStatus {
        available,
        authenticated: available, // SSH is always "authenticated" if available
        account: None,
    }
}

async fn cloud_status(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<CloudStatus>, StatusCode> {
    let (azure, aws, gcp, ssh) = tokio::join!(check_azure(), check_aws(), check_gcp(), check_ssh());

    Ok(Json(CloudStatus {
        azure,
        aws,
        gcp,
        ssh,
    }))
}

/// Project-scoped cloud status (pass-through — cloud status is global).
pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/cloud/status", get(cloud_status))
        .with_state(state)
}
