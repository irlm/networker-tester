//! Conformance suite pinned to `shared/sdk-contract-v1.json` (contract §9).
//!
//! These tests parse the machine-readable contract twin and drive the tower
//! service directly via `ServiceExt::oneshot`, asserting route shapes,
//! envelopes, headers, clamping, invisibility, and the 429/Retry-After path.
//!
//! Requires the `axum` feature (they mount via `laghound::router`). Run with
//! `cargo test --all-features`.
#![cfg(feature = "axum")]

use std::net::{IpAddr, Ipv4Addr};

use axum::body::Body;
use http::{Request, Response, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::{Service, ServiceExt};

use laghound::{ClientIp, Config};

const TOKEN: &str = "conformance-token-0123456789";

/// Load and parse the machine-readable contract twin.
fn contract() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../shared/sdk-contract-v1.json"
    );
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&raw).expect("contract JSON parses")
}

/// A fresh axum router built from `laghound::router`, with the inner-most
/// service being LagHound's fallback. Each call gets its own limiter state.
fn app(cfg: Config) -> axum::Router {
    laghound::router(cfg).expect("valid config mounts")
}

fn base_config() -> Config {
    Config::new(TOKEN)
}

fn req(method: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .extension(ClientIp(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))))
        .body(Body::empty())
        .unwrap()
}

fn authed(method: &str, uri: &str) -> Request<Body> {
    let mut r = req(method, uri);
    r.headers_mut()
        .insert("x-laghound-token", TOKEN.parse().unwrap());
    r
}

async fn send(app: &mut axum::Router, request: Request<Body>) -> Response<Body> {
    ServiceExt::<Request<Body>>::ready(app)
        .await
        .unwrap()
        .call(request)
        .await
        .unwrap()
}

async fn body_json(resp: Response<Body>) -> Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

async fn oneshot(cfg: Config, request: Request<Body>) -> Response<Body> {
    app(cfg).oneshot(request).await.unwrap()
}

// ---------------------------------------------------------------------------
// Contract twin drives these assertions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn contract_version_is_v1() {
    let c = contract();
    assert_eq!(c["contract"], "v1");
    assert_eq!(c["prefix_default"], "/laghound");
    // Rust is a declared SDK language.
    let langs = c["sdk_langs"].as_array().unwrap();
    assert!(langs.iter().any(|v| v == "rust"));
}

#[tokio::test]
async fn every_route_in_contract_responds_authenticated() {
    let c = contract();
    for route in c["routes"].as_array().unwrap() {
        let method = route["method"].as_str().unwrap();
        let path = route["path"].as_str().unwrap();
        let uri = format!("/laghound{path}");
        let resp = oneshot(base_config(), authed(method, &uri)).await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "route {method} {uri} should be 200 when authenticated"
        );
        // Success responses carry Server-Timing + Cache-Control (§3).
        assert!(
            resp.headers().contains_key("server-timing"),
            "{uri} Server-Timing"
        );
        let cc = resp
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert_eq!(
            cc, "no-store, no-cache, must-revalidate",
            "{uri} Cache-Control"
        );
    }
}

#[tokio::test]
async fn app_metric_on_every_response() {
    for (m, p) in [
        ("GET", "/health"),
        ("GET", "/echo"),
        ("GET", "/info"),
        ("GET", "/download"),
        ("POST", "/upload"),
    ] {
        let resp = oneshot(base_config(), authed(m, &format!("/laghound{p}"))).await;
        let st = resp
            .headers()
            .get("server-timing")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            st.contains("app;dur="),
            "{p} must emit app metric, got: {st}"
        );
    }
}

#[tokio::test]
async fn health_shape_matches_contract() {
    let resp = oneshot(
        base_config().app_name("checkout-api"),
        authed("GET", "/laghound/health"),
    )
    .await;
    let body = body_json(resp).await;
    assert_eq!(body["contract"], "v1");
    assert_eq!(body["status"], "ok");
    assert_eq!(body["sdk"]["lang"], "rust");
    assert!(body["sdk"]["version"].is_string());
    assert_eq!(body["app"], "checkout-api");
    assert!(body["uptime_s"].is_u64());
    for r in ["health", "echo", "download", "upload", "info"] {
        assert!(body["routes"][r].is_boolean(), "routes.{r}");
    }
    assert_eq!(body["routes"]["health"], true);
}

#[tokio::test]
async fn echo_body_is_fixed() {
    let c = contract();
    let expected = c["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "echo")
        .unwrap()["response"]["body_fixed"]
        .clone();
    let resp = oneshot(base_config(), authed("GET", "/laghound/echo")).await;
    let body = body_json(resp).await;
    assert_eq!(body, expected);
}

#[tokio::test]
async fn info_never_leaks_token() {
    let resp = oneshot(base_config(), authed("GET", "/laghound/info")).await;
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(!text.contains(TOKEN), "token must never appear in /info");
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["token_set"], true);
    assert_eq!(body["caps"]["absolute_max_bytes"], 33_554_432u64);
    assert_eq!(body["prefix"], "/laghound");
}

#[tokio::test]
async fn download_clamps_and_reports_via_header() {
    // Ask for 8 MiB with a 1 MiB cap -> clamped to 1 MiB, reported in header.
    let cfg = base_config().download_cap(1_048_576);
    let resp = oneshot(cfg, authed("GET", "/laghound/download?bytes=8388608")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "application/octet-stream"
    );
    let reported: u64 = resp
        .headers()
        .get("x-laghound-bytes")
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(reported, 1_048_576);
    let cl: u64 = resp
        .headers()
        .get("content-length")
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(cl, 1_048_576);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.len() as u64, 1_048_576);
    assert!(body.iter().all(|&b| b == 0x42), "fill byte must be 0x42");
}

#[tokio::test]
async fn download_absolute_max_enforced_over_config() {
    // Config asks for 64 MiB cap; absolute max is 32 MiB.
    let cfg = base_config().download_cap(64 * 1024 * 1024);
    let resp = oneshot(cfg, authed("GET", "/laghound/download?bytes=100000000")).await;
    let reported: u64 = resp
        .headers()
        .get("x-laghound-bytes")
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(reported, 33_554_432, "clamped to the 32 MiB absolute max");
}

#[tokio::test]
async fn download_invalid_param_is_400() {
    let resp = oneshot(base_config(), authed("GET", "/laghound/download?bytes=-5")).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp).await;
    assert_eq!(body["contract"], "v1");
    assert_eq!(body["error"]["code"], "invalid_param");
    // Fixed message — must not echo the offending value.
    assert!(!body["error"]["message"].as_str().unwrap().contains("-5"));
}

#[tokio::test]
async fn upload_content_length_over_cap_is_413_without_reading() {
    let cfg = base_config().upload_cap(1024);
    let mut r = authed("POST", "/laghound/upload");
    // Declare a Content-Length over the cap but send no body: a 413 without
    // reading proves the SDK does not block on the (absent) body.
    r.headers_mut()
        .insert("content-length", "2048".parse().unwrap());
    let resp = oneshot(cfg, r).await;
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = body_json(resp).await;
    assert_eq!(body["error"]["code"], "payload_too_large");
}

#[tokio::test]
async fn upload_counts_and_reports_received_bytes() {
    let payload = vec![0u8; 4096];
    let mut r = authed("POST", "/laghound/upload");
    *r.body_mut() = Body::from(payload.clone());
    let resp = oneshot(base_config(), r).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let received: u64 = resp
        .headers()
        .get("x-laghound-bytes")
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(received, 4096);
    let st = resp
        .headers()
        .get("server-timing")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let body = body_json(resp).await;
    assert_eq!(body["received_bytes"], 4096);
    // recv + app both present on upload (§4.2).
    assert!(
        st.contains("recv;dur="),
        "upload Server-Timing has recv: {st}"
    );
    assert!(
        st.contains("app;dur="),
        "upload Server-Timing has app: {st}"
    );
}

// ---- 404-invisibility (contract §5, §6.5) ----------------------------------

#[tokio::test]
async fn bad_token_is_bare_404_all_routes_including_health() {
    for (m, p) in [
        ("GET", "/health"),
        ("GET", "/echo"),
        ("GET", "/info"),
        ("GET", "/download"),
        ("POST", "/upload"),
    ] {
        let mut r = req(m, &format!("/laghound{p}"));
        r.headers_mut()
            .insert("x-laghound-token", "wrong".parse().unwrap());
        let resp = oneshot(base_config(), r).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{p} bad token -> 404");
        // Bare: no LagHound headers, no Server-Timing, no WWW-Authenticate.
        assert!(!resp.headers().contains_key("server-timing"), "{p} bare");
        assert!(!resp.headers().contains_key("cache-control"), "{p} bare");
        assert!(!resp.headers().contains_key("www-authenticate"), "{p} bare");
        assert!(!resp.headers().contains_key("x-laghound-bytes"), "{p} bare");
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert!(bytes.is_empty(), "{p} bare 404 has empty body");
    }
}

#[tokio::test]
async fn missing_token_is_bare_404() {
    let resp = oneshot(base_config(), req("GET", "/laghound/health")).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(!resp.headers().contains_key("server-timing"));
}

// The kill-switch (`LAGHOUND_DISABLED`) is process-global env state; its test
// lives in its own binary (`tests/killswitch.rs`) so mutating the variable
// never races another parallel test's env read.

#[tokio::test]
async fn bearer_token_authenticates() {
    let mut r = req("GET", "/laghound/health");
    r.headers_mut()
        .insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
    let resp = oneshot(base_config(), r).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn x_laghound_token_wins_over_authorization() {
    // Correct X-LagHound-Token + a wrong Bearer: X wins, so it authenticates.
    let mut r = req("GET", "/laghound/health");
    r.headers_mut()
        .insert("x-laghound-token", TOKEN.parse().unwrap());
    r.headers_mut()
        .insert("authorization", "Bearer nope".parse().unwrap());
    let resp = oneshot(base_config(), r).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn unknown_subpath_under_prefix_is_bare_404() {
    let resp = oneshot(base_config(), authed("GET", "/laghound/nope")).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(!resp.headers().contains_key("server-timing"));
}

#[tokio::test]
async fn wrong_method_on_known_route_is_405() {
    let resp = oneshot(base_config(), authed("POST", "/laghound/echo")).await;
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    let body = body_json(resp).await;
    assert_eq!(body["error"]["code"], "method_not_allowed");
}

// ---- rate limit / 429 + Retry-After (contract §6.2) ------------------------

#[tokio::test]
async fn authenticated_rate_limit_is_429_with_retry_after() {
    // Tiny bucket so we exhaust it deterministically from a single IP.
    let cfg = base_config().rate_per_ip(1, 2).rate_global(1000, 1000);
    let mut app = app(cfg);
    let mut last = None;
    for _ in 0..6 {
        last = Some(send(&mut app, authed("GET", "/laghound/echo")).await);
    }
    let resp = last.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        resp.headers().contains_key("retry-after"),
        "429 must carry Retry-After"
    );
    let body = body_json(resp).await;
    assert_eq!(body["error"]["code"], "rate_limited");
    assert!(body["error"]["retry_after_ms"].is_u64());
}

#[tokio::test]
async fn unauthenticated_rate_limit_is_bare_404_not_429() {
    // Rate limit runs before auth; unauth rejections stay invisible (§5).
    let cfg = base_config().rate_per_ip(1, 2).rate_global(1000, 1000);
    let mut app = app(cfg);
    let mut last = None;
    for _ in 0..6 {
        // No token.
        last = Some(send(&mut app, req("GET", "/laghound/echo")).await);
    }
    let resp = last.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "unauth limiter -> bare 404"
    );
    assert!(!resp.headers().contains_key("retry-after"));
    assert!(!resp.headers().contains_key("server-timing"));
}

// ---- pass-through: non-prefix paths are untouched --------------------------

#[tokio::test]
async fn non_prefix_paths_pass_through_to_app() {
    // Merge laghound into an app that owns "/".
    let app = axum::Router::new()
        .route("/", axum::routing::get(|| async { "app root" }))
        .merge(app(base_config()));
    let resp = app.oneshot(req("GET", "/")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"app root");
}
