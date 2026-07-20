//! The core tower [`Service`] (and its [`Layer`]) implementing LagHound
//! contract v1. Requests under the configured prefix are handled here; all
//! others pass through to the inner service.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use bytes::Buf as _;
use http::{Request, Response, StatusCode};
use http_body::Body;
use http_body_util::BodyExt;
use tower::{Layer, Service};

use crate::config::{Config, ConfigError, ENV_DISABLED};
use crate::error::{envelope, ErrorCode};
use crate::limits::RateOutcome;
use crate::resp::{bare_404, download_response, json_response, LagBody};
use crate::state::State;
use crate::timing::Metric;

/// A tower extension carrying the peer IP for per-IP rate limiting (contract
/// §6.2: IP is the socket peer, never `X-Forwarded-For` unless configured).
/// The axum router sugar inserts this from `ConnectInfo`; users of the bare
/// [`LagHoundService`] should insert it themselves for accurate per-IP limits.
#[derive(Clone, Copy, Debug)]
pub struct ClientIp(pub IpAddr);

/// A tower [`Layer`] that wraps a service with LagHound endpoint handling.
#[derive(Clone)]
pub struct LagHoundLayer {
    state: State,
    disabled_cache: Arc<DisabledCache>,
}

impl LagHoundLayer {
    /// Build the layer from a validated [`Config`] (fail-closed on bad config).
    pub fn new(config: Config) -> Result<Self, ConfigError> {
        let config = config.build()?;
        Ok(Self {
            state: State::from_config(config),
            disabled_cache: Arc::new(DisabledCache::new()),
        })
    }
}

impl<S> Layer<S> for LagHoundLayer {
    type Service = LagHoundService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        LagHoundService {
            inner,
            state: self.state.clone(),
            disabled_cache: self.disabled_cache.clone(),
        }
    }
}

/// The tower [`Service`] produced by [`LagHoundLayer`].
#[derive(Clone)]
pub struct LagHoundService<S> {
    inner: S,
    state: State,
    disabled_cache: Arc<DisabledCache>,
}

impl<S> LagHoundService<S> {
    /// Wrap an inner service directly (equivalent to applying [`LagHoundLayer`]).
    pub fn new(inner: S, config: Config) -> Result<Self, ConfigError> {
        Ok(LagHoundLayer::new(config)?.layer(inner))
    }
}

/// Caches the `LAGHOUND_DISABLED` env read for <= 1s (contract §6.5).
struct DisabledCache {
    checked_at: AtomicU64, // millis since process start
    value: AtomicBool,
    origin: Instant,
}

impl DisabledCache {
    fn new() -> Self {
        Self {
            checked_at: AtomicU64::new(0),
            value: AtomicBool::new(read_disabled_env()),
            origin: Instant::now(),
        }
    }

    fn is_disabled(&self) -> bool {
        let now_ms = self.origin.elapsed().as_millis() as u64;
        let last = self.checked_at.load(Ordering::Relaxed);
        if now_ms.saturating_sub(last) >= 1000 || last == 0 {
            let v = read_disabled_env();
            self.value.store(v, Ordering::Relaxed);
            self.checked_at.store(now_ms.max(1), Ordering::Relaxed);
            v
        } else {
            self.value.load(Ordering::Relaxed)
        }
    }
}

fn read_disabled_env() -> bool {
    std::env::var(ENV_DISABLED)
        .map(|v| v == "1")
        .unwrap_or(false)
}

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for LagHoundService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    ReqBody: Body + Send + 'static,
    ReqBody::Data: Send,
    ReqBody::Error: Send,
    ResBody: Body<Data = bytes::Bytes> + Send + 'static,
{
    type Response = Response<EitherBody<ResBody>>;
    type Error = S::Error;
    type Future = BoxFuture<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        // Only intercept requests under the prefix; everything else passes to
        // the inner service untouched.
        if !path_under_prefix(req.uri().path(), &self.state.0.prefix) {
            let mut inner = self.inner.clone();
            return Box::pin(async move {
                let resp = inner.call(req).await?;
                Ok(resp.map(EitherBody::Inner))
            });
        }

        let state = self.state.clone();
        let disabled = self.disabled_cache.clone();
        Box::pin(async move {
            // Fail-closed: a panic inside LagHound code converts to a 500
            // envelope confined to the LagHound route (contract §6.7). It never
            // crashes the host process.
            let fut = std::panic::AssertUnwindSafe(handle(state, disabled, req));
            let resp = match futures_util::future::FutureExt::catch_unwind(fut).await {
                Ok(resp) => resp,
                Err(_) => envelope(ErrorCode::Internal, None, &[]),
            };
            Ok(resp.map(EitherBody::Lag))
        })
    }
}

fn path_under_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|r| r.starts_with('/'))
}

/// The body a [`LagHoundService`] returns: either the inner service's body or
/// LagHound's own [`LagBody`].
pub enum EitherBody<B> {
    Inner(B),
    Lag(LagBody),
}

impl<B> Body for EitherBody<B>
where
    B: Body<Data = bytes::Bytes>,
{
    type Data = bytes::Bytes;
    type Error = EitherError<B::Error>;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        // Safety: standard structural pin projection on an enum.
        unsafe {
            match self.get_unchecked_mut() {
                EitherBody::Inner(b) => Pin::new_unchecked(b)
                    .poll_frame(cx)
                    .map_err(EitherError::Inner),
                EitherBody::Lag(b) => Pin::new_unchecked(b)
                    .poll_frame(cx)
                    .map_err(EitherError::Lag),
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            EitherBody::Inner(b) => b.is_end_stream(),
            EitherBody::Lag(b) => b.is_end_stream(),
        }
    }

    fn size_hint(&self) -> http_body::SizeHint {
        match self {
            EitherBody::Inner(b) => b.size_hint(),
            EitherBody::Lag(b) => b.size_hint(),
        }
    }
}

/// Error type for [`EitherBody`].
#[derive(Debug)]
pub enum EitherError<E> {
    Inner(E),
    Lag(std::convert::Infallible),
}

impl<E: std::fmt::Display> std::fmt::Display for EitherError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EitherError::Inner(e) => write!(f, "{e}"),
            EitherError::Lag(e) => match *e {},
        }
    }
}

impl<E: std::error::Error> std::error::Error for EitherError<E> {}

fn client_ip<B>(req: &Request<B>) -> IpAddr {
    req.extensions()
        .get::<ClientIp>()
        .map(|c| c.0)
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
}

/// The full request pipeline (contract §5 check order):
/// kill switch -> rate/concurrency limits -> auth -> route logic.
async fn handle<ReqBody>(
    state: State,
    disabled: Arc<DisabledCache>,
    req: Request<ReqBody>,
) -> Response<LagBody>
where
    ReqBody: Body + Send + 'static,
    ReqBody::Data: Send,
    ReqBody::Error: Send,
{
    // 1. Kill switch -> bare 404 (contract §6.5).
    if disabled.is_disabled() {
        return bare_404();
    }

    let ip = client_ip(&req);
    let authed = is_authed(&state, &req);

    // 2. Rate limits BEFORE auth (contract §5). Unauthenticated limiter
    //    rejections are bare 404s to stay invisible; authenticated ones are
    //    429 + Retry-After.
    if let RateOutcome::Limited { retry_after } = state.0.rate.check(ip) {
        if authed {
            let ms = retry_after.as_millis() as u64;
            return envelope(ErrorCode::RateLimited, Some(ms.max(1)), &[]);
        }
        return bare_404();
    }

    // 3. Auth. Bad/missing token -> bare 404 (contract §5), including /health.
    if !authed {
        return bare_404();
    }

    // Resolve the route (path after the prefix).
    let path = req.uri().path().to_string();
    let sub = path.strip_prefix(&state.0.prefix).unwrap_or("");
    // Strip a trailing query already handled by uri().path(); sub is the route.
    let route = sub.split('?').next().unwrap_or(sub);

    // 4. Concurrency cap (authenticated traffic only reaches here) -> 429.
    //    A download route counts as a transfer; its permit is moved into the
    //    streamed body so the transfer slot is held for the whole transfer.
    let is_transfer = matches!(route, "/download" | "/upload");
    let permit = match state.0.concurrency.acquire(is_transfer) {
        Some(p) => p,
        None => return envelope(ErrorCode::RateLimited, Some(1000), &[]),
    };

    // 5. Route logic.
    match route {
        "/health" => route_health(&state, req.method()),
        "/echo" if state.0.routes.echo => route_echo(req).await,
        "/echo" => bare_404(),
        "/download" if state.0.routes.download => route_download(&state, &req, permit),
        "/download" => bare_404(),
        "/upload" if state.0.routes.upload => route_upload(&state, req).await,
        "/upload" => bare_404(),
        "/info" if state.0.routes.info => route_info(&state, req.method()),
        "/info" => bare_404(),
        // Unknown subpath under prefix -> bare 404 (contract §7).
        _ => bare_404(),
    }
    // `permit` is dropped here for all non-download routes (upload's transfer
    // slot is held only while draining, which happens inside route_upload
    // before this point returns — acceptable since upload buffers nothing).
}

/// Extract the presented token and compare constant-time (contract §5).
fn is_authed<B>(state: &State, req: &Request<B>) -> bool {
    let headers = req.headers();
    // X-LagHound-Token wins; the other is ignored (not compared).
    if let Some(v) = headers.get("x-laghound-token") {
        return state.token_matches(v.as_bytes());
    }
    if let Some(v) = headers.get(http::header::AUTHORIZATION) {
        if let Ok(s) = v.to_str() {
            if let Some(tok) = s.strip_prefix("Bearer ") {
                return state.token_matches(tok.as_bytes());
            }
        }
    }
    false
}

fn app_timing(dur_ms: f64) -> Vec<Metric> {
    // `app` + `total` compat alias (contract §4.2). total == app here.
    let mut v = Vec::new();
    if let Some(m) = Metric::new("app", dur_ms) {
        v.push(m);
    }
    if let Some(m) = Metric::new("total", dur_ms) {
        v.push(m);
    }
    v
}

fn route_health(state: &State, method: &http::Method) -> Response<LagBody> {
    if method != http::Method::GET {
        return envelope(ErrorCode::MethodNotAllowed, None, &app_timing(0.0));
    }
    let start = Instant::now();
    let mut body = state.0.health_template.clone();
    body["uptime_s"] = serde_json::json!(state.uptime_s());
    let dur = start.elapsed().as_secs_f64() * 1000.0;
    json_response(StatusCode::OK, &body, &app_timing(dur), &[])
}

fn route_info(state: &State, method: &http::Method) -> Response<LagBody> {
    if method != http::Method::GET {
        return envelope(ErrorCode::MethodNotAllowed, None, &app_timing(0.0));
    }
    let start = Instant::now();
    let mut body = state.0.info_template.clone();
    body["uptime_s"] = serde_json::json!(state.uptime_s());
    let dur = start.elapsed().as_secs_f64() * 1000.0;
    json_response(StatusCode::OK, &body, &app_timing(dur), &[])
}

async fn route_echo<ReqBody>(req: Request<ReqBody>) -> Response<LagBody>
where
    ReqBody: Body + Send + 'static,
    ReqBody::Data: Send,
    ReqBody::Error: Send,
{
    if req.method() != http::Method::GET {
        return envelope(ErrorCode::MethodNotAllowed, None, &app_timing(0.0));
    }
    let start = Instant::now();
    // Reject bodies > 64 KiB (contract §3.2). Prefer Content-Length; otherwise
    // count while draining, aborting once the cap is exceeded.
    let (parts, body) = req.into_parts();
    if let Some(len) = content_length(&parts.headers) {
        if len > crate::config::ECHO_REQUEST_BODY_MAX_BYTES {
            return envelope(ErrorCode::PayloadTooLarge, None, &app_timing(0.0));
        }
    }
    match drain_capped(body, crate::config::ECHO_REQUEST_BODY_MAX_BYTES).await {
        DrainResult::Ok(_) => {}
        DrainResult::OverCap => {
            return envelope(ErrorCode::PayloadTooLarge, None, &app_timing(0.0))
        }
        DrainResult::Error => return envelope(ErrorCode::Internal, None, &app_timing(0.0)),
    }
    // Fixed, byte-constant body — zero reflection (contract §3.2).
    let dur = start.elapsed().as_secs_f64() * 1000.0;
    let body = serde_json::json!({ "contract": "v1", "ok": true });
    json_response(StatusCode::OK, &body, &app_timing(dur), &[])
}

fn route_download<ReqBody>(
    state: &State,
    req: &Request<ReqBody>,
    permit: crate::limits::Permit,
) -> Response<LagBody> {
    if req.method() != http::Method::GET {
        // Method error drops `permit` here — no transfer slot leaked.
        return envelope(ErrorCode::MethodNotAllowed, None, &app_timing(0.0));
    }
    let start = Instant::now();
    let requested = match parse_bytes_param(req.uri().query()) {
        Ok(v) => v.unwrap_or(crate::config::DEFAULT_CAP_BYTES),
        Err(()) => return envelope(ErrorCode::InvalidParam, None, &app_timing(0.0)),
    };
    // Effective = min(N, download_cap, absolute_max) (contract §3.3).
    let actual = requested.min(state.effective_download_cap());
    // Optional byte budget applies to transfer routes (contract §6.4).
    if let Some(budget) = &state.0.budget {
        if let Err(retry) = budget.reserve(actual) {
            let ms = retry.as_millis() as u64;
            return envelope(ErrorCode::RateLimited, Some(ms.max(1)), &app_timing(0.0));
        }
    }
    let body = state.0.fill.body(actual);
    // app measures setup time only, before the first chunk (contract §3.3).
    let dur = start.elapsed().as_secs_f64() * 1000.0;
    // Move the permit into the streamed body so the transfer slot is held for
    // the whole transfer (contract §6.3).
    download_response(body, actual, &app_timing(dur), Some(permit))
}

async fn route_upload<ReqBody>(state: &State, req: Request<ReqBody>) -> Response<LagBody>
where
    ReqBody: Body + Send + 'static,
    ReqBody::Data: Send,
    ReqBody::Error: Send,
{
    if req.method() != http::Method::POST {
        return envelope(ErrorCode::MethodNotAllowed, None, &app_timing(0.0));
    }
    let cap = state.effective_upload_cap();
    let (parts, body) = req.into_parts();

    // Content-Length over cap -> immediate 413 WITHOUT reading the body (§3.4).
    if let Some(len) = content_length(&parts.headers) {
        if len > cap {
            return envelope(ErrorCode::PayloadTooLarge, None, &recv_app_timing(0.0, 0.0));
        }
    }

    // Optional byte budget (contract §6.4): if the window is already exhausted,
    // refuse before reading the body. Otherwise reserve the declared length (or
    // the cap when the length is unknown) so a burst cannot over-transfer.
    if let Some(budget) = &state.0.budget {
        let reserve = content_length(&parts.headers)
            .map(|l| l.min(cap))
            .unwrap_or(cap);
        if let Err(retry) = budget.reserve(reserve) {
            let ms = retry.as_millis() as u64;
            return envelope(
                ErrorCode::RateLimited,
                Some(ms.max(1)),
                &recv_app_timing(0.0, 0.0),
            );
        }
    }

    // Drain-and-count; abort at cap for chunked/unknown-length bodies (§3.4).
    let recv_start = Instant::now();
    let received = match drain_capped(body, cap).await {
        DrainResult::Ok(n) => n,
        DrainResult::OverCap => {
            return envelope(ErrorCode::PayloadTooLarge, None, &recv_app_timing(0.0, 0.0))
        }
        DrainResult::Error => {
            return envelope(ErrorCode::Internal, None, &recv_app_timing(0.0, 0.0))
        }
    };
    let recv_ms = recv_start.elapsed().as_secs_f64() * 1000.0;

    let app_start = Instant::now();
    let body = serde_json::json!({ "contract": "v1", "received_bytes": received });
    let app_ms = app_start.elapsed().as_secs_f64() * 1000.0;
    json_response(
        StatusCode::OK,
        &body,
        &recv_app_timing(recv_ms, app_ms),
        &[("x-laghound-bytes", received.to_string())],
    )
}

/// `recv;dur=..., app;dur=..., total;dur=recv+app` (contract §4.2 upload row).
fn recv_app_timing(recv_ms: f64, app_ms: f64) -> Vec<Metric> {
    let mut v = Vec::new();
    if let Some(m) = Metric::new("recv", recv_ms) {
        v.push(m);
    }
    if let Some(m) = Metric::new("app", app_ms) {
        v.push(m);
    }
    if let Some(m) = Metric::new("total", recv_ms + app_ms) {
        v.push(m);
    }
    v
}

fn content_length(headers: &http::HeaderMap) -> Option<u64> {
    headers
        .get(http::header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

/// Parse `?bytes=N` (contract §3.3). Missing -> `Ok(None)`; present-but-bad or
/// negative -> `Err(())` -> `400 invalid_param`.
fn parse_bytes_param(query: Option<&str>) -> Result<Option<u64>, ()> {
    let Some(q) = query else { return Ok(None) };
    for pair in q.split('&') {
        let mut kv = pair.splitn(2, '=');
        if kv.next() == Some("bytes") {
            let val = kv.next().unwrap_or("");
            // Negative or non-numeric -> invalid (a silent default would lie).
            return val.parse::<u64>().map(Some).map_err(|_| ());
        }
    }
    Ok(None)
}

enum DrainResult {
    Ok(u64),
    OverCap,
    Error,
}

/// Drain a body, counting bytes, never buffering more than one frame at a time
/// (peak memory O(chunk), contract §3.4/§6.6). Aborts once `cap` is exceeded.
async fn drain_capped<B>(body: B, cap: u64) -> DrainResult
where
    B: Body,
{
    let mut body = std::pin::pin!(body);
    let mut total: u64 = 0;
    loop {
        match body.as_mut().frame().await {
            Some(Ok(frame)) => {
                if let Some(data) = frame.data_ref() {
                    total = total.saturating_add(data.remaining() as u64);
                    if total > cap {
                        return DrainResult::OverCap;
                    }
                }
                // Frame is dropped here; nothing accumulated.
            }
            Some(Err(_)) => return DrainResult::Error,
            None => return DrainResult::Ok(total),
        }
    }
}
