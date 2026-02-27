/// All HTTP route handlers for the diagnostics endpoint.
use axum::{
    body::Body,
    extract::{Path, Query},
    http::{HeaderMap, Request, StatusCode, Version},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use bytes::Bytes;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::time::{sleep, Duration};

// ─────────────────────────────────────────────────────────────────────────────
// Router
// ─────────────────────────────────────────────────────────────────────────────

pub fn build_router() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/echo", post(echo).get(echo_get))
        .route("/download", get(download))
        .route("/upload", post(upload))
        .route("/delay", get(delay))
        .route("/headers", get(headers_echo))
        .route("/status/:code", get(status_code))
        .route("/http-version", get(http_version))
        .route("/info", get(server_info))
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// GET /health → 200 JSON { "status": "ok", "timestamp": "..." }
async fn health() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "timestamp": Utc::now().to_rfc3339(),
        "service": "networker-endpoint",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// GET /echo – returns empty body with request info
async fn echo_get(headers: HeaderMap) -> impl IntoResponse {
    let hdrs: HashMap<String, String> = headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    Json(serde_json::json!({
        "method": "GET",
        "headers": hdrs,
        "body_bytes": 0,
    }))
}

/// POST /echo – echoes the request body back in the response
async fn echo(headers: HeaderMap, body: Bytes) -> impl IntoResponse {
    let body_len = body.len();
    let hdrs: HashMap<String, String> = headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    // Return the body + a JSON envelope in the headers
    let resp = Response::builder()
        .status(200)
        .header("content-type", "application/octet-stream")
        .header("x-echo-body-bytes", body_len.to_string())
        .header("x-echo-received-headers", hdrs.len().to_string());

    // If the body is small enough to be UTF-8 JSON, return it directly;
    // otherwise return raw bytes.
    if body_len <= 1_048_576 {
        resp.body(Body::from(body)).unwrap()
    } else {
        Response::builder()
            .status(413)
            .body(Body::from("Payload too large (> 1 MiB)"))
            .unwrap()
    }
}

#[derive(Deserialize)]
struct DownloadParams {
    bytes: Option<usize>,
}

/// GET /download?bytes=N – returns N zero bytes  (max 100 MiB)
async fn download(Query(p): Query<DownloadParams>) -> impl IntoResponse {
    let n = p.bytes.unwrap_or(1024).min(104_857_600); // cap 100 MiB
    let body = vec![0u8; n];
    Response::builder()
        .status(200)
        .header("content-type", "application/octet-stream")
        .header("content-length", n.to_string())
        .header("x-download-bytes", n.to_string())
        .body(Body::from(body))
        .unwrap()
}

#[derive(Serialize)]
struct UploadStats {
    received_bytes: usize,
    timestamp: String,
}

/// POST /upload – accepts body, returns stats JSON
async fn upload(body: Bytes) -> impl IntoResponse {
    Json(UploadStats {
        received_bytes: body.len(),
        timestamp: Utc::now().to_rfc3339(),
    })
}

#[derive(Deserialize)]
struct DelayParams {
    ms: Option<u64>,
}

/// GET /delay?ms=N – sleeps N ms (max 30 s) then returns 200
async fn delay(Query(p): Query<DelayParams>) -> impl IntoResponse {
    let ms = p.ms.unwrap_or(0).min(30_000);
    sleep(Duration::from_millis(ms)).await;
    Json(serde_json::json!({
        "delayed_ms": ms,
        "timestamp": Utc::now().to_rfc3339(),
    }))
}

/// GET /headers – returns all received request headers as JSON
async fn headers_echo(headers: HeaderMap) -> impl IntoResponse {
    let map: HashMap<String, String> = headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    Json(map)
}

/// GET /status/:code – returns the specified HTTP status code
async fn status_code(Path(code): Path<u16>) -> impl IntoResponse {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST);
    (
        status,
        Json(serde_json::json!({
            "status": code,
            "description": status.canonical_reason().unwrap_or("Unknown"),
        })),
    )
}

/// GET /http-version – returns the HTTP version used by the client
async fn http_version(req: Request<Body>) -> impl IntoResponse {
    let version = match req.version() {
        Version::HTTP_09 => "HTTP/0.9",
        Version::HTTP_10 => "HTTP/1.0",
        Version::HTTP_11 => "HTTP/1.1",
        Version::HTTP_2 => "HTTP/2",
        Version::HTTP_3 => "HTTP/3",
        _ => "Unknown",
    };
    Json(serde_json::json!({
        "version": version,
        "timestamp": Utc::now().to_rfc3339(),
    }))
}

/// GET /info – server capabilities
async fn server_info() -> impl IntoResponse {
    Json(serde_json::json!({
        "service": "networker-endpoint",
        "version": env!("CARGO_PKG_VERSION"),
        "protocols": ["HTTP/1.1", "HTTP/2"],
        "http3": false,
        "endpoints": [
            "/health", "/echo", "/download", "/upload",
            "/delay", "/headers", "/status/:code", "/http-version", "/info"
        ],
        "timestamp": Utc::now().to_rfc3339(),
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::to_bytes, http::Request};
    use tower::ServiceExt; // for `oneshot`

    fn app() -> Router {
        build_router()
    }

    #[tokio::test]
    async fn health_returns_200() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn echo_returns_body() {
        let payload = b"hello world".as_ref();
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/echo")
                    .header("content-type", "text/plain")
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], payload);
    }

    #[tokio::test]
    async fn download_returns_requested_bytes() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/download?bytes=256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(body.len(), 256);
    }

    #[tokio::test]
    async fn status_endpoint_returns_404() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/status/404")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
    }

    #[tokio::test]
    async fn status_endpoint_returns_503() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/status/503")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 503);
    }

    #[tokio::test]
    async fn delay_endpoint_responds() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/delay?ms=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn http_version_responds() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/http-version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 512).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["version"].is_string());
    }

    #[tokio::test]
    async fn headers_endpoint_echoes_headers() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/headers")
                    .header("x-test-header", "networker")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["x-test-header"], "networker");
    }
}
