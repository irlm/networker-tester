use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::AppState;

#[derive(Deserialize)]
pub struct CreateDeploymentRequest {
    pub name: String,
    pub config: serde_json::Value, // The deploy.json content from wizard
}

#[derive(Serialize)]
pub struct CreateDeploymentResponse {
    pub deployment_id: Uuid,
    pub status: String,
}

#[derive(Deserialize)]
pub struct ListDeploymentsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

async fn create_deployment(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateDeploymentRequest>,
) -> Result<Json<CreateDeploymentResponse>, StatusCode> {
    // Build provider summary from config
    let provider_summary = build_provider_summary(&req.config);

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in create_deployment");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let deployment_id = crate::db::deployments::create(
        &client,
        &req.name,
        &req.config,
        provider_summary.as_deref(),
        None,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to insert deployment into DB");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    tracing::info!(
        deployment_id = %deployment_id,
        name = %req.name,
        "Deployment created, starting runner"
    );

    // Spawn the deployment runner in a background task
    let events_tx = state.events_tx.clone();
    let db_pool = Arc::new(state.db.clone());
    let config = req.config.clone();
    tokio::spawn(async move {
        match crate::deploy::runner::run_deployment(deployment_id, &config, events_tx, db_pool)
            .await
        {
            Ok(ips) => {
                tracing::info!(
                    deployment_id = %deployment_id,
                    endpoint_ips = ?ips,
                    "Deployment completed successfully"
                );
            }
            Err(e) => {
                tracing::error!(
                    deployment_id = %deployment_id,
                    error = %e,
                    "Deployment failed"
                );
            }
        }
    });

    Ok(Json(CreateDeploymentResponse {
        deployment_id,
        status: "pending".into(),
    }))
}

async fn list_deployments(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListDeploymentsQuery>,
) -> Result<Json<Vec<crate::db::deployments::DeploymentRow>>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in list_deployments");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let deployments =
        crate::db::deployments::list(&client, q.limit.unwrap_or(50), q.offset.unwrap_or(0))
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to list deployments from DB");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    Ok(Json(deployments))
}

async fn get_deployment(
    State(state): State<Arc<AppState>>,
    Path(deployment_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in get_deployment");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let deployment = crate::db::deployments::get(&client, &deployment_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get deployment from DB");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or_else(|| {
            tracing::warn!(deployment_id = %deployment_id, "Deployment not found");
            StatusCode::NOT_FOUND
        })?;

    Ok(Json(serde_json::to_value(deployment).unwrap_or_default()))
}

async fn stop_deployment(
    State(state): State<Arc<AppState>>,
    Path(deployment_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in stop_deployment");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let deployment = crate::db::deployments::get(&client, &deployment_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get deployment for stop");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    if deployment.status == "running" || deployment.status == "pending" {
        crate::db::deployments::update_status(&client, &deployment_id, "cancelled")
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to update deployment status to cancelled");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        let _ = state
            .events_tx
            .send(networker_common::messages::DashboardEvent::DeployComplete {
                deployment_id,
                status: "cancelled".into(),
                endpoint_ips: vec![],
            });
        tracing::info!(deployment_id = %deployment_id, "Deployment cancelled");
    }

    Ok(Json(serde_json::json!({"status": "cancelled"})))
}

async fn delete_deployment(
    State(state): State<Arc<AppState>>,
    Path(deployment_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in delete_deployment");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let deleted = crate::db::deployments::delete(&client, &deployment_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to delete deployment from DB");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if deleted {
        Ok(Json(serde_json::json!({"deleted": true})))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn check_deployment(
    State(state): State<Arc<AppState>>,
    Path(deployment_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in check_deployment");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let deployment = crate::db::deployments::get(&client, &deployment_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get deployment for check");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let ips: Vec<String> = deployment
        .endpoint_ips
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    let http_client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    // Get latest release version
    let latest_release: Option<String> = async {
        let c = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .ok()?;
        let resp = c
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
    .await;

    let mut results = Vec::new();
    for ip in &ips {
        // Try HTTP health endpoint to get version
        let health_url = format!("https://{ip}:8443/health");
        let (alive, version) = match http_client.get(&health_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let body: serde_json::Value = resp.json().await.unwrap_or_default();
                let ver = body
                    .get("version")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                (true, ver)
            }
            _ => {
                // Try plain TCP as fallback
                let addr = format!("{ip}:8443");
                let alive = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    tokio::net::TcpStream::connect(&addr),
                )
                .await
                .map(|r| r.is_ok())
                .unwrap_or(false);
                (alive, None)
            }
        };

        let outdated = match (&version, &latest_release) {
            (Some(v), Some(latest)) => v != latest,
            _ => false,
        };

        results.push(serde_json::json!({
            "ip": ip,
            "alive": alive,
            "version": version,
            "outdated": outdated,
        }));
    }

    Ok(Json(serde_json::json!({
        "endpoints": results,
        "latest_release": latest_release,
    })))
}

async fn update_endpoint(
    State(state): State<Arc<AppState>>,
    Path(deployment_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in update_endpoint");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let deployment = crate::db::deployments::get(&client, &deployment_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get deployment for update");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Generate a deploy.json for endpoint-only update (no tests)
    let mut update_config = deployment.config.clone();
    update_config["tests"] = serde_json::json!({"run_tests": false});

    let events_tx = state.events_tx.clone();
    let db_pool = Arc::new(state.db.clone());

    tracing::info!(deployment_id = %deployment_id, "Starting endpoint update");

    // Spawn the update in background using the deploy runner
    tokio::spawn(async move {
        match crate::deploy::runner::run_deployment(
            deployment_id,
            &update_config,
            events_tx,
            db_pool,
        )
        .await
        {
            Ok(_) => tracing::info!(deployment_id = %deployment_id, "Endpoint update completed"),
            Err(e) => {
                tracing::error!(deployment_id = %deployment_id, error = %e, "Endpoint update failed")
            }
        }
    });

    Ok(Json(serde_json::json!({"status": "updating"})))
}

/// Build a human-readable summary of providers from deploy.json config.
fn build_provider_summary(config: &serde_json::Value) -> Option<String> {
    let endpoints = config.get("endpoints")?.as_array()?;
    let mut parts: Vec<String> = Vec::new();

    for ep in endpoints {
        let provider = ep
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let region = ep
            .get("region")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if region.is_empty() {
            parts.push(provider.to_string());
        } else {
            parts.push(format!("{provider} {region}"));
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" + "))
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/deployments",
            get(list_deployments).post(create_deployment),
        )
        .route(
            "/deployments/:deployment_id",
            get(get_deployment).delete(delete_deployment),
        )
        .route("/deployments/:deployment_id/stop", post(stop_deployment))
        .route("/deployments/:deployment_id/check", post(check_deployment))
        .route("/deployments/:deployment_id/update", post(update_endpoint))
        .with_state(state)
}
