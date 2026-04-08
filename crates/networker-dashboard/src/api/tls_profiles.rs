use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::ProjectContext;
use crate::AppState;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

#[derive(Deserialize)]
pub struct ListTlsProfilesQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

async fn list_tls_profiles(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListTlsProfilesQuery>,
    req: axum::extract::Request,
) -> Result<Json<Vec<crate::db::tls_profiles::TlsProfileSummaryRow>>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = q.offset.unwrap_or(0).max(0);
    let runs = crate::db::tls_profiles::list(&client, &ctx.project_id, limit, offset)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(runs))
}

async fn get_tls_profile(
    State(state): State<Arc<AppState>>,
    Path((_, run_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<Json<crate::db::tls_profiles::TlsProfileDetail>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let run = crate::db::tls_profiles::get(&client, &ctx.project_id, &run_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(run))
}

pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/tls-profiles", get(list_tls_profiles))
        .route("/tls-profiles/{run_id}", get(get_tls_profile))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::{ListTlsProfilesQuery, DEFAULT_LIMIT, MAX_LIMIT};

    #[test]
    fn clamp_limit_and_offset_behave_safely() {
        let q = ListTlsProfilesQuery {
            limit: Some(9999),
            offset: Some(-5),
        };
        let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let offset = q.offset.unwrap_or(0).max(0);
        assert_eq!(limit, MAX_LIMIT);
        assert_eq!(offset, 0);
    }
}
