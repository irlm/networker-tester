use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::AppState;

#[derive(Deserialize)]
pub struct ListUrlTestsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

async fn list_url_tests(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListUrlTestsQuery>,
) -> Result<Json<Vec<crate::db::url_tests::UrlTestSummary>>, StatusCode> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let runs = crate::db::url_tests::list(&client, q.limit.unwrap_or(50), q.offset.unwrap_or(0))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(runs))
}

async fn get_url_test(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<Uuid>,
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
    Path(run_id): Path<Uuid>,
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

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/url-tests", get(list_url_tests))
        .route("/url-tests/:run_id", get(get_url_test))
        .route("/url-tests/:run_id/sections", get(get_url_test_sections))
        .with_state(state)
}
