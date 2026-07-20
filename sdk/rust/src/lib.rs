//! # LagHound SDK (Rust)
//!
//! Embed a tiny diagnostic endpoint into your axum/tower app; the LagHound
//! multi-cloud tester fleet then measures your *real* app from outside and
//! splits total request time into
//! `DNS -> TCP -> TLS -> network transfer -> server processing`. The last
//! phase — server processing — is reported by this SDK via a
//! `Server-Timing: app;dur=<ms>` header stamped on every response. That split
//! is the whole point.
//!
//! This crate implements **endpoint contract v1** (`docs/sdk/contract-v1.md`,
//! `shared/sdk-contract-v1.json`). It is a tower [`Layer`](tower::Layer) /
//! [`Service`](tower::Service); with the `axum` feature it also exposes
//! [`router`] for one-line mounting.
//!
//! ## Quickstart (axum)
//!
//! ```no_run
//! # #[cfg(feature = "axum")]
//! # async fn demo() {
//! let token = std::env::var("LAGHOUND_TOKEN").unwrap();
//! let app = axum::Router::new()
//!     .merge(laghound::router(laghound::Config::new(token)).unwrap());
//! # let _ = app;
//! # }
//! ```
//!
//! ## Safety (contract §6)
//!
//! Hard 32 MiB byte cap, per-IP + global token-bucket rate limits, an overall
//! concurrency cap and a transfer concurrency cap, an optional sliding-window
//! byte budget, a `LAGHOUND_DISABLED=1` kill switch, constant-time token
//! comparison, and 404-invisibility — a bad/missing token, a rate-limited
//! unauthenticated request, or the kill switch all return a bare, header-less
//! 404 indistinguishable from "route not found". Zero body/token logging, zero
//! reflection of request input.

mod body;
mod config;
mod error;
mod limits;
mod resp;
mod service;
mod state;
mod timing;

#[cfg(feature = "axum")]
mod axum_router;

pub use config::{ByteBudget, Config, ConfigError, RateLimit, RouteToggles, SDK_LANG, SDK_VERSION};
pub use service::{ClientIp, EitherBody, LagHoundLayer, LagHoundService};

#[cfg(feature = "axum")]
pub use axum_router::router;

/// Build a bare tower [`Service`](tower::Service) wrapping `inner` with the
/// LagHound endpoint (fail-closed on invalid config). Prefer [`router`] under
/// axum; use this to compose with any other tower stack.
pub fn service<S>(inner: S, config: Config) -> Result<LagHoundService<S>, ConfigError> {
    LagHoundService::new(inner, config)
}

/// Build the tower [`Layer`](tower::Layer) (fail-closed on invalid config).
pub fn layer(config: Config) -> Result<LagHoundLayer, ConfigError> {
    LagHoundLayer::new(config)
}
