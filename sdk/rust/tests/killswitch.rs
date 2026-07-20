//! The `LAGHOUND_DISABLED` kill switch (contract §6.5). Isolated in its own
//! test binary: it is the only test that mutates that process-global env var,
//! so no parallel test's env read can observe the mutation.
//!
//! Requires the `axum` feature. Run with `cargo test --all-features`.
#![cfg(feature = "axum")]

use std::net::{IpAddr, Ipv4Addr};

use axum::body::Body;
use http::{Request, StatusCode};
use tower::ServiceExt;

use laghound::{ClientIp, Config};

const TOKEN: &str = "killswitch-token-0123456789";

fn authed(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .extension(ClientIp(IpAddr::V4(Ipv4Addr::LOCALHOST)))
        .header("x-laghound-token", TOKEN)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn kill_switch_makes_everything_bare_404() {
    // Baseline: enabled -> /health is 200.
    std::env::remove_var("LAGHOUND_DISABLED");
    let app = laghound::router(Config::new(TOKEN)).unwrap();
    let ok = app.oneshot(authed("/laghound/health")).await.unwrap();
    assert_eq!(ok.status(), StatusCode::OK);

    // Flip the kill switch -> bare 404, no Server-Timing.
    std::env::set_var("LAGHOUND_DISABLED", "1");
    let app = laghound::router(Config::new(TOKEN)).unwrap();
    let off = app.oneshot(authed("/laghound/health")).await.unwrap();
    std::env::remove_var("LAGHOUND_DISABLED");
    assert_eq!(off.status(), StatusCode::NOT_FOUND);
    assert!(!off.headers().contains_key("server-timing"));
    assert!(!off.headers().contains_key("cache-control"));
}
