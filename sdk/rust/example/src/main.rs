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
//! curl -s http://localhost:8084/                       # -> "rust sample ok"
//! curl -s http://localhost:8084/work                   # -> ~30ms of work
//! curl -si -H "X-LagHound-Token: demo-token-laghound" \
//!      http://localhost:8084/laghound/health           # -> 200 + Server-Timing
//! curl -si http://localhost:8084/laghound/health       # -> bare 404 (no token)
//! ```

use std::net::SocketAddr;
use std::time::Duration;

use axum::{routing::get, Router};

#[tokio::main]
async fn main() {
    let token = std::env::var("LAGHOUND_TOKEN").unwrap_or_else(|_| "demo-token-laghound".into());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8084);

    // The LagHound endpoint, mounted at its default `/laghound` prefix. Fails
    // closed if the token is missing/too short — here it always has a default.
    let laghound = laghound::router(laghound::Config::new(token).app_name("laghound-rust-sample"))
        .expect("laghound config is valid");

    let app = Router::new()
        .route("/", get(|| async { "rust sample ok" }))
        .route(
            "/work",
            get(|| async {
                // ~30ms of "server processing" so the split has something to show.
                tokio::time::sleep(Duration::from_millis(30)).await;
                "did ~30ms of work"
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
