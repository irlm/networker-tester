//! Minimal axum service that nests the LagHound endpoint at `/laghound` and
//! serves two app routes of its own. Run it, then point the LagHound tester
//! fleet (or `curl`) at `http://localhost:8084/laghound/echo` with the token.
//!
//! Env:
//!   - `LAGHOUND_TOKEN` — shared secret (default `demo-token-laghound`).
//!   - `PORT`           — listen port (default `8084`).
//!
//! ```text
//! cargo run
//! curl -s http://localhost:8084/                       # -> interactive demo page
//! curl -s http://localhost:8084/work                   # -> ~30ms of work
//! curl -si -H "X-LagHound-Token: demo-token-laghound" \
//!      http://localhost:8084/laghound/health           # -> 200 + Server-Timing
//! curl -si http://localhost:8084/laghound/health       # -> bare 404 (no token)
//! ```

use std::net::SocketAddr;
use std::time::Duration;

use axum::{response::Html, routing::get, Router};

#[tokio::main]
async fn main() {
    let token = std::env::var("LAGHOUND_TOKEN").unwrap_or_else(|_| "demo-token-laghound".into());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8084);
    let public_demo = std::env::var("LAGHOUND_PUBLIC_DEMO").is_ok_and(|v| v == "1");

    // The LagHound endpoint, mounted at its default `/laghound` prefix. Fails
    // closed if the token is missing/too short — here it always has a default.
    let mut config = laghound::Config::new(token).app_name("laghound-rust-demo");
    if public_demo {
        // A known public-demo token should not expose bandwidth-heavy transfer
        // routes. Local/conformance use keeps the complete contract enabled.
        config = config
            .routes(laghound::RouteToggles {
                echo: true,
                download: false,
                upload: false,
                info: true,
            })
            .rate_per_ip(2, 5)
            .rate_global(10, 20);
    }
    let laghound = laghound::router(config).expect("laghound config is valid");

    let app = Router::new()
        .route("/", get(|| async { Html(include_str!("index.html")) }))
        .route(
            "/work",
            get(|| async {
                // ~30ms of "server processing" so the split has something to show.
                tokio::time::sleep(Duration::from_millis(30)).await;
                "Rust handler completed in about 30 ms"
            }),
        )
        .merge(laghound);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    println!("laghound-sample listening on http://{addr}  (LagHound at /laghound)");

    // `into_make_service_with_connect_info` gives LagHound the real peer IP for
    // per-IP rate limiting (contract §6.2 — it never trusts X-Forwarded-For).
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("server error");
}
