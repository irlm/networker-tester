/// All HTTP route handlers for the diagnostics endpoint.
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, Version},
    middleware::{self, Next},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use bytes::Bytes;
use chrono::Utc;
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;
use tokio::time::{sleep, Duration};
use tower_http::trace::TraceLayer;

// ─────────────────────────────────────────────────────────────────────────────
// Router
// ─────────────────────────────────────────────────────────────────────────────

/// Build the router.
///
/// `h3_port` — when `Some(port)`, every response includes
/// `Alt-Svc: h3=":port"; ma=86400` so that Chrome can discover H3 support
/// and upgrade to QUIC on subsequent navigations.  Pass `None` when H3 is not
/// compiled in (the `http3` feature is disabled).
pub fn build_router(h3_port: Option<u16>) -> Router {
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
        .route("/page", get(page_manifest))
        .route("/browser-page", get(browser_page))
        .route("/asset", get(asset_handler))
        // Remove axum's default 2 MiB body limit so upload probes of arbitrary
        // size are not rejected with 413 before the body is transmitted.
        .layer(DefaultBodyLimit::disable())
        // Add X-Networker-Server-Timestamp (and optionally Alt-Svc) to every response.
        .layer(middleware::from_fn_with_state(
            h3_port,
            add_server_timestamp,
        ))
        // Log every request (method + URI) and response (status + latency).
        // Verbosity is controlled by RUST_LOG; defaults to INFO.
        .layer(TraceLayer::new_for_http())
}

// ─────────────────────────────────────────────────────────────────────────────
// Middleware
// ─────────────────────────────────────────────────────────────────────────────

/// Middleware that stamps every response with the server wall-clock time, version,
/// and (when `h3_port` is set) an `Alt-Svc` header advertising HTTP/3 support.
///
/// The `Alt-Svc` header is served on all responses regardless of scheme.
/// Chrome ignores it for plain-HTTP origins; it only upgrades to QUIC when
/// the header arrives over HTTPS — exactly the behaviour we want.
async fn add_server_timestamp(
    State(h3_port): State<Option<u16>>,
    req: Request,
    next: Next,
) -> Response {
    let mut response = next.run(req).await;
    let ts = Utc::now().to_rfc3339();
    if let Ok(val) = HeaderValue::from_str(&ts) {
        response
            .headers_mut()
            .insert("x-networker-server-timestamp", val);
    }
    response.headers_mut().insert(
        "x-networker-server-version",
        HeaderValue::from_static(env!("CARGO_PKG_VERSION")),
    );
    // Advertise H3 so Chrome can upgrade to QUIC on the next request to this origin.
    if let Some(port) = h3_port {
        let alt_svc = format!("h3=\":{port}\"; ma=86400");
        if let Ok(val) = HeaderValue::from_str(&alt_svc) {
            response.headers_mut().insert("alt-svc", val);
        }
    }
    response
}

// ─────────────────────────────────────────────────────────────────────────────
// Context-switch helpers (Unix only)
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `(voluntary_csw, involuntary_csw)` for the server process.
#[cfg(unix)]
fn csw_snapshot() -> (i64, i64) {
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
    (u.ru_nvcsw, u.ru_nivcsw)
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

/// GET /download?bytes=N – returns N zero bytes (max 2 GiB).
/// Adds `Server-Timing: proc;dur=X, csw-v;dur=N, csw-i;dur=N` indicating
/// body generation time and context switches.
async fn download(Query(p): Query<DownloadParams>) -> impl IntoResponse {
    let n = p.bytes.unwrap_or(1024).min(2 * 1024 * 1024 * 1024); // cap 2 GiB
    let t0 = Instant::now();
    #[cfg(unix)]
    let (csw_v0, csw_i0) = csw_snapshot();
    let body = vec![0u8; n];
    let proc_ms = t0.elapsed().as_secs_f64() * 1000.0;
    #[cfg(unix)]
    let csw_part = {
        let (csw_v1, csw_i1) = csw_snapshot();
        format!(
            ", csw-v;dur={}, csw-i;dur={}",
            csw_v1 - csw_v0,
            csw_i1 - csw_i0
        )
    };
    #[cfg(not(unix))]
    let csw_part = "";

    let timing = format!("proc;dur={proc_ms:.3}{csw_part}");
    Response::builder()
        .status(200)
        .header("content-type", "application/octet-stream")
        .header("content-length", n.to_string())
        .header("x-download-bytes", n.to_string())
        .header("server-timing", timing.as_str())
        .body(Body::from(body))
        .unwrap()
}

#[derive(Serialize)]
struct UploadStats {
    received_bytes: usize,
    timestamp: String,
}

/// POST /upload – drains the request body without buffering it in memory,
/// then returns a JSON stats object with the byte count.
///
/// Adds `Server-Timing: recv;dur=X` (body drain time) and echoes
/// `X-Networker-Request-Id` from the request if present.
/// Adds `X-Networker-Received-Bytes` with the actual drained byte count so the
/// client can verify the upload was not silently truncated.
async fn upload(req: Request) -> impl IntoResponse {
    // Extract request metadata before consuming the body.
    let request_id = req
        .headers()
        .get("x-networker-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned());

    let t0 = Instant::now();
    #[cfg(unix)]
    let (csw_v0, csw_i0) = csw_snapshot();
    let mut received_bytes: usize = 0;
    let mut body = req.into_body();
    while let Some(Ok(frame)) = body.frame().await {
        if let Ok(data) = frame.into_data() {
            received_bytes += data.len();
        }
    }
    let recv_ms = t0.elapsed().as_secs_f64() * 1000.0;
    #[cfg(unix)]
    let csw_part = {
        let (csw_v1, csw_i1) = csw_snapshot();
        format!(
            ", csw-v;dur={}, csw-i;dur={}",
            csw_v1 - csw_v0,
            csw_i1 - csw_i0
        )
    };
    #[cfg(not(unix))]
    let csw_part = "";

    let mut resp = Json(UploadStats {
        received_bytes,
        timestamp: Utc::now().to_rfc3339(),
    })
    .into_response();

    let timing = format!("recv;dur={recv_ms:.3}{csw_part}");
    if let Ok(v) = HeaderValue::from_str(&timing) {
        resp.headers_mut().insert("server-timing", v);
    }
    // Always echo the actual received byte count as a response header so the
    // client can detect upload truncation without parsing the JSON body.
    resp.headers_mut().insert(
        "x-networker-received-bytes",
        HeaderValue::from(received_bytes as u64),
    );
    if let Some(rid) = request_id {
        if let Ok(v) = HeaderValue::from_str(&rid) {
            resp.headers_mut().insert("x-networker-request-id", v);
        }
    }

    resp
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
async fn http_version(req: Request) -> impl IntoResponse {
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
        "protocols": if cfg!(feature = "http3") {
            serde_json::json!(["HTTP/1.1", "HTTP/2", "HTTP/3"])
        } else {
            serde_json::json!(["HTTP/1.1", "HTTP/2"])
        },
        "http3": cfg!(feature = "http3"),
        "endpoints": [
            "/health", "/echo", "/download", "/upload",
            "/delay", "/headers", "/status/:code", "/http-version", "/info"
        ],
        "timestamp": Utc::now().to_rfc3339(),
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Page-load simulation routes
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct PageParams {
    assets: Option<usize>,
    bytes: Option<usize>,
}

#[derive(Deserialize)]
struct AssetParams {
    #[allow(dead_code)]
    id: Option<u32>,
    bytes: Option<usize>,
}

/// GET /page?assets=N&bytes=B → JSON manifest listing N asset URLs.
async fn page_manifest(Query(p): Query<PageParams>) -> impl IntoResponse {
    let n = p.assets.unwrap_or(20).min(500);
    let b = p.bytes.unwrap_or(10_240);
    let assets: Vec<String> = (0..n).map(|i| format!("/asset?id={i}&bytes={b}")).collect();
    Json(serde_json::json!({
        "asset_count": n,
        "asset_bytes": b,
        "assets": assets,
    }))
}

/// GET /browser-page?assets=N&bytes=B → HTML page with N `<img>` tags pointing to /asset.
///
/// Each img src triggers a real HTTP fetch; the browser's `load` event fires only after
/// all images have settled (loaded or errored), making this suitable for measuring full
/// page-load time with a real browser (chromiumoxide / CDP).
async fn browser_page(Query(p): Query<PageParams>) -> impl IntoResponse {
    let n = p.assets.unwrap_or(20).min(500);
    let b = p.bytes.unwrap_or(10_240);

    let mut html = String::from(
        "<!DOCTYPE html>\n\
         <html><head><title>Networker Page Load Test</title></head>\n\
         <body>\n",
    );
    for i in 0..n {
        html.push_str(&format!(
            "<img src=\"/asset?id={i}&bytes={b}\" width=\"1\" height=\"1\" alt=\"\">\n"
        ));
    }
    html.push_str("</body></html>\n");

    Response::builder()
        .status(200)
        .header("content-type", "text/html; charset=utf-8")
        .body(Body::from(html))
        .unwrap()
}

/// GET /asset?id=X&bytes=B → B zero bytes, content-type: application/octet-stream.
async fn asset_handler(Query(p): Query<AssetParams>) -> impl IntoResponse {
    let n = p.bytes.unwrap_or(10_240).min(100 * 1024 * 1024); // cap 100 MiB
    Response::builder()
        .status(200)
        .header("content-type", "application/octet-stream")
        .header("content-length", n.to_string())
        .body(Body::from(vec![0u8; n]))
        .unwrap()
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
        build_router(None)
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
    async fn health_has_server_timestamp() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.headers().contains_key("x-networker-server-timestamp"),
            "server timestamp header missing"
        );
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
    async fn download_has_server_timing() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/download?bytes=64")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.headers().contains_key("server-timing"),
            "server-timing header missing from download"
        );
    }

    #[tokio::test]
    async fn upload_echoes_request_id() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/upload")
                    .header("x-networker-request-id", "test-id-123")
                    .body(Body::from(b"data".as_ref()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers()
                .get("x-networker-request-id")
                .and_then(|v| v.to_str().ok()),
            Some("test-id-123"),
            "x-networker-request-id not echoed"
        );
        assert!(
            resp.headers().contains_key("server-timing"),
            "server-timing header missing from upload"
        );
    }

    #[tokio::test]
    async fn upload_returns_received_bytes_header() {
        let payload = b"hello world 12345";
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/upload")
                    .body(Body::from(payload.as_ref()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let received: usize = resp
            .headers()
            .get("x-networker-received-bytes")
            .expect("x-networker-received-bytes header missing")
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            received,
            payload.len(),
            "received-bytes header must match body size"
        );
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
