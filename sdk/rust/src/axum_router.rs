//! `axum` feature: `laghound::router(config)` -> an [`axum::Router`] you can
//! `.merge()` / `.nest()` into your app.
//!
//! The returned router handles every LagHound route under the configured
//! prefix and returns a bare 404 for anything else it is asked to serve, so it
//! is safe to `.merge()` alongside your own routes.

use std::net::SocketAddr;

use axum::extract::connect_info::ConnectInfo;
use axum::Router;
use http::{Request, Response, StatusCode};
use tower::{Layer, Service};

use crate::config::{Config, ConfigError};
use crate::service::{ClientIp, EitherBody, LagHoundLayer};

/// Build an [`axum::Router`] serving the LagHound endpoint (contract v1).
///
/// ```no_run
/// let token = std::env::var("LAGHOUND_TOKEN").unwrap();
/// let app = axum::Router::new()
///     .merge(laghound::router(laghound::Config::new(token)).unwrap());
/// ```
///
/// To get accurate per-IP rate limiting, serve with
/// `into_make_service_with_connect_info::<SocketAddr>()` — the router reads the
/// peer address from axum's `ConnectInfo` and never trusts `X-Forwarded-For`
/// (contract §6.2).
pub fn router(config: Config) -> Result<Router, ConfigError> {
    let layer = LagHoundLayer::new(config)?;

    // Inner fallback: a bare 404 for any path the layer does not claim.
    let inner = tower::service_fn(|_req: Request<axum::body::Body>| async move {
        Ok::<Response<axum::body::Body>, std::convert::Infallible>(
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(axum::body::Body::empty())
                .expect("static 404 is valid"),
        )
    });

    let lag = layer.layer(inner);

    // Adapt to axum: inject ClientIp from ConnectInfo and map the response body.
    let svc = tower::service_fn(move |mut req: Request<axum::body::Body>| {
        let mut lag = lag.clone();
        if let Some(ConnectInfo(addr)) = req.extensions().get::<ConnectInfo<SocketAddr>>().copied()
        {
            req.extensions_mut().insert(ClientIp(addr.ip()));
        }
        async move {
            let resp = Service::call(&mut lag, req).await?;
            Ok::<Response<axum::body::Body>, std::convert::Infallible>(
                resp.map(|b: EitherBody<axum::body::Body>| axum::body::Body::new(b)),
            )
        }
    });

    Ok(Router::new().fallback_service(svc))
}
