use axum::{
    extract::{Path, Request, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::{require_project_role, AuthUser, ProjectContext, ProjectRole};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct RegisterVmRequest {
    pub name: String,
    pub cloud: String,
    pub region: String,
    pub ip: String,
    #[serde(default = "default_ssh_user")]
    pub ssh_user: String,
    pub vm_size: Option<String>,
}

fn default_ssh_user() -> String {
    "azureuser".to_string()
}

fn request_extension<T>(req: &Request, name: &'static str) -> Result<T, StatusCode>
where
    T: Clone + Send + Sync + 'static,
{
    req.extensions().get::<T>().cloned().ok_or_else(|| {
        tracing::error!(extension = name, "Missing required request extension");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

/// GET /projects/{pid}/benchmark-catalog
async fn list_catalog(
    State(state): State<Arc<AppState>>,
    req: Request,
) -> Result<Json<Vec<crate::db::benchmark_vm_catalog::VmCatalogRow>>, StatusCode> {
    let ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in list_benchmark_catalog");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let vms = crate::db::benchmark_vm_catalog::list_for_project(&client, &ctx.project_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to list VM catalog");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(vms))
}

/// POST /projects/{pid}/benchmark-catalog
async fn register_vm(
    State(state): State<Arc<AppState>>,
    req: Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
    let user = request_extension::<AuthUser>(&req, "AuthUser")?;
    require_project_role(&ctx, ProjectRole::Operator)?;

    let body = axum::body::to_bytes(req.into_body(), 32 * 1024)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let payload: RegisterVmRequest =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    if payload.name.is_empty() || payload.ip.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in register_benchmark_vm");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let vm_id = crate::db::benchmark_vm_catalog::create(
        &client,
        &ctx.project_id,
        &payload.name,
        &payload.cloud,
        &payload.region,
        &payload.ip,
        &payload.ssh_user,
        payload.vm_size.as_deref(),
        Some(&user.user_id),
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to register VM in catalog");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    tracing::info!(
        audit_event = "benchmark_vm_registered",
        project_id = %ctx.project_id,
        user_id = %user.user_id,
        vm_id = %vm_id,
        name = %payload.name,
        cloud = %payload.cloud,
        ip = %payload.ip,
        "Benchmark VM registered in catalog"
    );

    Ok(Json(serde_json::json!({"vm_id": vm_id})))
}

/// DELETE /projects/{pid}/benchmark-catalog/:vm_id
async fn remove_vm(
    State(state): State<Arc<AppState>>,
    Path((_, vm_id)): Path<(String, Uuid)>,
    req: Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
    let user = request_extension::<AuthUser>(&req, "AuthUser")?;
    require_project_role(&ctx, ProjectRole::Operator)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in remove_benchmark_vm");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Verify VM belongs to project
    let vm = crate::db::benchmark_vm_catalog::get(&client, &vm_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get VM for deletion");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    if vm.project_id != ctx.project_id {
        return Err(StatusCode::NOT_FOUND);
    }

    crate::db::benchmark_vm_catalog::delete(&client, &vm_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to delete VM from catalog");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    tracing::info!(
        audit_event = "benchmark_vm_removed",
        project_id = %ctx.project_id,
        user_id = %user.user_id,
        vm_id = %vm_id,
        "Benchmark VM removed from catalog"
    );

    Ok(Json(serde_json::json!({"ok": true})))
}

/// POST /projects/{pid}/benchmark-catalog/{vm_id}/detect
/// SSH to the VM and detect which languages are deployed.
async fn detect_languages(
    State(state): State<Arc<AppState>>,
    Path((_, vm_id)): Path<(String, Uuid)>,
    req: Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ctx = request_extension::<ProjectContext>(&req, "ProjectContext")?;
    let _user = request_extension::<AuthUser>(&req, "AuthUser")?;
    require_project_role(&ctx, ProjectRole::Operator)?;

    let client = state.db.get().await.map_err(|e| {
        tracing::error!(error = %e, "DB pool error in detect_languages");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let vm = crate::db::benchmark_vm_catalog::get(&client, &vm_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get VM for language detection");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    if vm.project_id != ctx.project_id {
        return Err(StatusCode::NOT_FOUND);
    }

    // SSH to VM and detect languages
    let languages = ssh_detect_languages(&vm.ip, &vm.ssh_user).await;

    let languages_json = serde_json::to_value(&languages).unwrap_or_default();
    crate::db::benchmark_vm_catalog::update_languages(&client, &vm_id, &languages_json)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to update VM languages");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Also mark as online since SSH succeeded (or offline if empty/error)
    let status = if languages.is_empty() {
        "offline"
    } else {
        "online"
    };
    crate::db::benchmark_vm_catalog::update_status(&client, &vm_id, status)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to update VM status");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    tracing::info!(
        vm_id = %vm_id,
        ip = %vm.ip,
        language_count = languages.len(),
        languages = ?languages,
        "Language detection complete"
    );

    Ok(Json(
        serde_json::json!({"languages": languages, "status": status}),
    ))
}

/// SSH to a VM and check for known language binaries/files.
async fn ssh_detect_languages(ip: &str, ssh_user: &str) -> Vec<String> {
    let checks = vec![
        ("rust", "test -f /opt/bench/rust-server"),
        ("go", "test -f /opt/bench/go-server"),
        ("cpp", "test -f /opt/bench/cpp-build/server"),
        ("nodejs", "test -f /opt/bench/nodejs/server.js"),
        ("python", "test -f /opt/bench/python/server.py"),
        ("ruby", "test -f /opt/bench/ruby/config.ru"),
        ("php", "test -f /opt/bench/php/server.php"),
        ("java", "test -f /opt/bench/java/server.jar"),
        ("nginx", "which nginx > /dev/null 2>&1"),
    ];

    let mut detected = Vec::new();

    for (lang, cmd) in &checks {
        let output = tokio::process::Command::new("ssh")
            .args([
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "BatchMode=yes",
                &format!("{ssh_user}@{ip}"),
                cmd,
            ])
            .output()
            .await;

        if let Ok(out) = output {
            if out.status.success() {
                detected.push(lang.to_string());
            }
        }
    }

    // Check for C# .NET versions (multiple possible dirs)
    let csharp_output = tokio::process::Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "BatchMode=yes",
            &format!("{ssh_user}@{ip}"),
            "ls -d /opt/bench/csharp-net* 2>/dev/null | sed 's|/opt/bench/||'",
        ])
        .output()
        .await;

    if let Ok(out) = csharp_output {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            for line in stdout.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() && trimmed.starts_with("csharp-net") {
                    detected.push(trimmed.to_string());
                }
            }
        }
    }

    detected
}

pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/benchmark-catalog", get(list_catalog).post(register_vm))
        .route("/benchmark-catalog/{vm_id}", delete(remove_vm))
        .route("/benchmark-catalog/{vm_id}/detect", post(detect_languages))
        .with_state(state)
}
