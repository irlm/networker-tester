use axum::{http::StatusCode, routing::get, Json, Router};
use std::sync::Arc;

use crate::AppState;

fn group_detail(label: &str) -> &'static str {
    match label {
        "Network" => "Low-level connection probes. Measures DNS resolution, TCP handshake, TLS negotiation, and UDP round-trip independently — isolates each layer of the network stack.",
        "HTTP" => "Full HTTP request timing across protocol versions. Each probe does DNS + TCP + TLS + HTTP request/response and reports TTFB, total duration, and negotiated protocol.",
        "Page Load (Native)" => "Loads a page with multiple assets using the Rust HTTP stack (no browser). Compares H1 (6 parallel connections), H2 (multiplexed), and H3 (QUIC). Fastest, no rendering overhead.",
        "Page Load (Browser)" => "Same page load test but using real Chrome headless. Includes rendering, JavaScript, and browser networking. Measures what users actually experience — DOM loaded, full load, bytes transferred.",
        "Throughput" => "Sustained transfer speed tests with configurable payload sizes. Measures download and upload bandwidth across different HTTP versions and transport protocols.",
        _ => "",
    }
}

async fn list_modes() -> Result<Json<serde_json::Value>, StatusCode> {
    let modes = networker_tester::metrics::Protocol::all_modes();

    let mut groups: Vec<serde_json::Value> = Vec::new();
    let mut current_group = String::new();
    let mut current_modes: Vec<serde_json::Value> = Vec::new();

    for mode in &modes {
        if mode.group != current_group {
            if !current_group.is_empty() {
                groups.push(serde_json::json!({
                    "label": current_group,
                    "detail": group_detail(&current_group),
                    "modes": current_modes,
                }));
                current_modes = Vec::new();
            }
            current_group = mode.group.clone();
        }
        current_modes.push(serde_json::json!({
            "id": mode.id,
            "name": mode.name,
            "desc": mode.description,
            "detail": mode.detail,
        }));
    }
    if !current_group.is_empty() {
        groups.push(serde_json::json!({
            "label": current_group,
            "detail": group_detail(&current_group),
            "modes": current_modes,
        }));
    }

    Ok(Json(serde_json::json!({ "groups": groups })))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/modes", get(list_modes))
        .with_state(state)
}
