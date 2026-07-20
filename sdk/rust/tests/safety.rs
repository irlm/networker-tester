//! Safety-property tests (contract §6, §9): fail-closed config, streaming
//! memory bound, byte budget 429, concurrency cap 429, and echo 413.
//!
//! Requires the `axum` feature. Run with `cargo test --all-features`.
#![cfg(feature = "axum")]

use std::net::{IpAddr, Ipv4Addr};

use axum::body::Body;
use http::{Request, Response, StatusCode};
use http_body::Body as _;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use laghound::{ClientIp, Config, ConfigError};

const TOKEN: &str = "safety-token-0123456789abcdef";

fn app(cfg: Config) -> axum::Router {
    laghound::router(cfg).expect("valid config mounts")
}

fn authed(method: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .extension(ClientIp(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))))
        .header("x-laghound-token", TOKEN)
        .body(Body::empty())
        .unwrap()
}

async fn oneshot(cfg: Config, request: Request<Body>) -> Response<Body> {
    app(cfg).oneshot(request).await.unwrap()
}

async fn body_json(resp: Response<Body>) -> Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

// ---- fail-closed config (contract §2) --------------------------------------

#[test]
fn from_env_without_token_refuses_to_mount() {
    // Ensure the env var is unset for this assertion.
    std::env::remove_var("LAGHOUND_TOKEN");
    assert_eq!(Config::from_env().unwrap_err(), ConfigError::MissingToken);
    // A token shorter than 16 bytes fails to build.
    assert_eq!(
        Config::new("short").build().unwrap_err(),
        ConfigError::TokenTooShort
    );
}

#[test]
fn token_min_length_enforced() {
    assert_eq!(
        Config::new("0123456789abcde").build().unwrap_err(), // 15 chars
        ConfigError::TokenTooShort
    );
    assert!(Config::new("0123456789abcdef").build().is_ok()); // 16 chars
}

#[test]
fn too_many_tokens_rejected() {
    let cfg = Config::new(TOKEN).add_token(TOKEN).add_token(TOKEN);
    assert_eq!(cfg.build().unwrap_err(), ConfigError::TooManyTokens);
}

#[test]
fn invalid_prefix_rejected() {
    assert_eq!(
        Config::new(TOKEN).prefix("laghound").build().unwrap_err(),
        ConfigError::InvalidPrefix
    );
    assert_eq!(
        Config::new(TOKEN).prefix("/laghound/").build().unwrap_err(),
        ConfigError::InvalidPrefix
    );
    assert!(Config::new(TOKEN).prefix("/lh").build().is_ok());
}

#[test]
fn router_fails_closed_on_bad_config() {
    assert!(laghound::router(Config::new("short")).is_err());
}

// ---- token rotation (contract §5) ------------------------------------------

#[tokio::test]
async fn previous_token_still_authenticates_during_rotation() {
    let cfg = Config::new("new-token-0123456789").add_token("old-token-0123456789");
    let mut r = Request::builder()
        .method("GET")
        .uri("/laghound/health")
        .extension(ClientIp(IpAddr::V4(Ipv4Addr::LOCALHOST)))
        .body(Body::empty())
        .unwrap();
    r.headers_mut()
        .insert("x-laghound-token", "old-token-0123456789".parse().unwrap());
    let resp = oneshot(cfg, r).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

// ---- streaming memory bound (contract §3.3, §9) ----------------------------

#[tokio::test]
async fn download_streams_in_bounded_frames() {
    // A 16 MiB download must arrive as many small frames (<= 64 KiB each),
    // proving no single O(N) buffer is produced. We assert each frame is
    // bounded rather than measuring RSS (which is what the reviewed §9
    // criterion checks in CI).
    let cfg = Config::new(TOKEN).download_cap(32 * 1024 * 1024);
    let resp = oneshot(cfg, authed("GET", "/laghound/download?bytes=16777216")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let mut body = resp.into_body();
    let mut total = 0u64;
    let mut max_frame = 0usize;
    loop {
        match std::future::poll_fn(|cx| std::pin::Pin::new(&mut body).poll_frame(cx)).await {
            Some(Ok(frame)) => {
                if let Some(d) = frame.data_ref() {
                    max_frame = max_frame.max(d.len());
                    total += d.len() as u64;
                }
            }
            Some(Err(_)) => panic!("body error"),
            None => break,
        }
    }
    assert_eq!(total, 16_777_216);
    assert!(
        max_frame <= 65_536,
        "frames must be <= 64 KiB, saw {max_frame}"
    );
}

// ---- byte budget 429 (contract §6.4) ---------------------------------------

#[tokio::test]
async fn byte_budget_exhaustion_is_429_with_retry_after() {
    // Budget of 5 MiB; each 4 MiB download reserves its transfer bytes, so the
    // second exhausts the window and is refused (contract §6.4).
    let cfg = Config::new(TOKEN)
        .download_cap(4 * 1024 * 1024)
        .byte_budget(5 * 1024 * 1024, 600);
    let mut app = app(cfg);
    use tower::Service;
    // First transfer succeeds (reserves 4 MiB of the 5 MiB window).
    let r1 = tower::ServiceExt::<Request<Body>>::ready(&mut app)
        .await
        .unwrap()
        .call(authed("GET", "/laghound/download?bytes=4194304"))
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::OK);
    // Drain it so the transfer permit releases (budget is separate state).
    let _ = r1.into_body().collect().await.unwrap();
    // Second would reserve another 4 MiB -> 8 > 5 -> 429.
    let r2 = tower::ServiceExt::<Request<Body>>::ready(&mut app)
        .await
        .unwrap()
        .call(authed("GET", "/laghound/download?bytes=4194304"))
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(r2.headers().contains_key("retry-after"));
    let body = body_json(r2).await;
    assert_eq!(body["error"]["code"], "rate_limited");
}

// ---- echo body cap 413 (contract §3.2, §6.1) -------------------------------

#[tokio::test]
async fn echo_rejects_oversize_body_with_413() {
    let big = vec![0u8; 70 * 1024]; // > 64 KiB
    let mut r = authed("GET", "/laghound/echo");
    *r.body_mut() = Body::from(big);
    let resp = oneshot(Config::new(TOKEN), r).await;
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

// ---- concurrency cap (contract §6.3) ---------------------------------------

#[tokio::test]
async fn transfer_concurrency_cap_returns_429() {
    // max_concurrent_transfers = 1: hold one download open, a second is 429.
    let cfg = Config::new(TOKEN)
        .max_concurrent(8)
        .max_concurrent_transfers(1)
        .download_cap(4 * 1024 * 1024);
    let mut app = app(cfg);
    use tower::Service;

    // Start a large download but DO NOT drain it — the permit is held until the
    // response body is dropped.
    let held = tower::ServiceExt::<Request<Body>>::ready(&mut app)
        .await
        .unwrap()
        .call(authed("GET", "/laghound/download?bytes=4194304"))
        .await
        .unwrap();
    assert_eq!(held.status(), StatusCode::OK);

    // A second transfer while the first is in-flight must be refused.
    let second = tower::ServiceExt::<Request<Body>>::ready(&mut app)
        .await
        .unwrap()
        .call(authed("GET", "/laghound/download?bytes=1024"))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(second.headers().contains_key("retry-after"));

    // Drop the held response -> permit released -> transfers allowed again.
    drop(held);
    let third = tower::ServiceExt::<Request<Body>>::ready(&mut app)
        .await
        .unwrap()
        .call(authed("GET", "/laghound/download?bytes=1024"))
        .await
        .unwrap();
    assert_eq!(third.status(), StatusCode::OK);
}

// ---- no reflection (contract §3.2, §6.6) -----------------------------------

#[tokio::test]
async fn echo_does_not_reflect_request_input() {
    let mut r = authed("GET", "/laghound/echo?evil=%3Cscript%3E");
    r.headers_mut()
        .insert("x-inject", "reflect-me-please".parse().unwrap());
    let resp = oneshot(Config::new(TOKEN), r).await;
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(!text.contains("evil"));
    assert!(!text.contains("reflect-me-please"));
    assert_eq!(text, r#"{"contract":"v1","ok":true}"#);
}

// ---- disabled route -> bare 404 (contract §3.1 routes map) -----------------

#[tokio::test]
async fn disabled_route_is_bare_404_but_health_reports_it() {
    let cfg = Config::new(TOKEN).routes(laghound::RouteToggles {
        echo: true,
        download: false,
        upload: true,
        info: true,
    });
    // /health reports download:false.
    let h = oneshot(cfg.clone(), authed("GET", "/laghound/health")).await;
    let hbody = body_json(h).await;
    assert_eq!(hbody["routes"]["download"], false);
    // /download itself is a bare 404.
    let d = oneshot(cfg, authed("GET", "/laghound/download")).await;
    assert_eq!(d.status(), StatusCode::NOT_FOUND);
    assert!(!d.headers().contains_key("server-timing"));
}
