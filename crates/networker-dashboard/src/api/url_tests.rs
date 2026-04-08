use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

use crate::auth::ProjectContext;
use crate::AppState;

#[derive(Deserialize)]
pub struct ListUrlTestsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

async fn get_url_test(
    State(state): State<Arc<AppState>>,
    Path((_, run_id)): Path<(String, Uuid)>,
) -> Result<Json<crate::db::url_tests::UrlTestDetail>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let run = crate::db::url_tests::get(&client, &run_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(run))
}

async fn get_url_test_sections(
    State(state): State<Arc<AppState>>,
    Path((_, run_id)): Path<(String, Uuid)>,
) -> Result<Json<crate::db::url_tests::UrlTestSectionedDetail>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let run = crate::db::url_tests::get(&client, &run_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(crate::db::url_tests::section_detail(run)))
}

// ── Project-scoped handlers ────────────────────────────────────────────

async fn list_url_tests_scoped(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListUrlTestsQuery>,
    req: axum::extract::Request,
) -> Result<Json<Vec<crate::db::url_tests::UrlTestSummary>>, StatusCode> {
    let ctx = req.extensions().get::<ProjectContext>().unwrap().clone();
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = q.offset.unwrap_or(0).max(0);
    let runs = crate::db::url_tests::list(&client, &ctx.project_id, limit, offset)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(runs))
}

pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/url-tests", get(list_url_tests_scoped))
        .route("/url-tests/{run_id}", get(get_url_test))
        .route("/url-tests/{run_id}/sections", get(get_url_test_sections))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::{ListUrlTestsQuery, DEFAULT_LIMIT, MAX_LIMIT};

    #[test]
    fn clamp_limit_and_offset_behave_safely() {
        let q = ListUrlTestsQuery {
            limit: Some(9999),
            offset: Some(-5),
        };
        let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let offset = q.offset.unwrap_or(0).max(0);
        assert_eq!(limit, MAX_LIMIT);
        assert_eq!(offset, 0);
    }
}
