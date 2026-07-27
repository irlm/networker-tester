//! Shared HTTP/2 load-generation machinery for the working-conditions modes.
//!
//! Both `responsiveness` (draft-ietf-ippm-responsiveness) and `mthroughput`
//! (multi-connection capacity) ramp parallel HTTP/2 connections that stream
//! the endpoint's `/download` / `/upload` routes until aggregate goodput
//! stabilizes under the same moving-average criterion. This module holds the
//! seams they share so the two runners cannot drift:
//!
//! - [`connect_h2`] — one H2 connection (TLS + ALPN `h2` for https, h2c prior
//!   knowledge for cleartext) with measured TCP/TLS handshake times AND a
//!   [`SocketProbe`] `dup(2)` of the raw fd taken BEFORE the stream is handed
//!   to rustls/hyper, so post-transfer kernel TCP stats (`tcp_info`) remain
//!   sampleable per connection;
//! - [`load_download_once`] / [`load_upload_once`] — one counted transfer,
//!   with upload bytes counted as the H2 layer polls them (back-pressured by
//!   flow control — the counter tracks what actually entered the send window);
//! - [`mean`] / [`stddev`] — the goodput-stability math.

use crate::metrics::Protocol;
use crate::runner::http::build_tls_config;
use crate::runner::socket_info::SocketProbe;
use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::body::Body as HyperBody;
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::ServerName;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tracing::debug;

/// Body type used on the shared H2 client connections.
pub type ProbeBody = BoxBody<Bytes, Infallible>;
/// Cloneable request sender for one H2 connection.
pub type H2Sender = hyper::client::conn::http2::SendRequest<ProbeBody>;

/// Connection parameters both load-generating modes resolve to.
#[derive(Debug, Clone)]
pub struct H2Target {
    /// Endpoint base URL (scheme selects h2-over-TLS vs h2c).
    pub base_url: url::Url,
    pub insecure: bool,
    pub ca_bundle: Option<String>,
    /// Per-connect timeout (ms).
    pub timeout_ms: u64,
}

/// Transfer direction of a load-generating connection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoadDirection {
    Download,
    Upload,
}

/// An established H2 connection plus its handshake timings and the dup'd
/// socket handle for post-transfer kernel stats.
pub struct H2Connection {
    pub sender: H2Sender,
    /// Measured TCP connect time (ms).
    pub tcp_ms: f64,
    /// Measured TLS handshake time (ms); `None` for cleartext (h2c) targets.
    pub tls_ms: Option<f64>,
    /// `dup(2)` of the connection's fd, taken before the TLS/hyper handover —
    /// `None` on non-Unix platforms or when `dup` fails.
    pub socket_probe: Option<SocketProbe>,
}

/// Establish an H2 connection to the endpoint (TLS + ALPN h2 for https, h2c
/// prior knowledge for http). The returned handshake timings feed the
/// responsiveness mode's foreign probes; the [`SocketProbe`] feeds the
/// mthroughput mode's per-connection TCP attribution.
pub async fn connect_h2(target: &H2Target) -> Result<H2Connection, String> {
    let url = &target.base_url;
    let host = url.host_str().ok_or("target URL has no host")?.to_string();
    let is_https = url.scheme() == "https";
    let port = url.port().unwrap_or(if is_https { 443 } else { 80 });
    let timeout = Duration::from_millis(target.timeout_ms.max(1));

    let t_tcp = Instant::now();
    let tcp = tokio::time::timeout(timeout, TcpStream::connect((host.as_str(), port)))
        .await
        .map_err(|_| format!("TCP connect timed out after {}ms", target.timeout_ms))?
        .map_err(|e| format!("TCP connect failed: {e}"))?;
    let tcp_ms = t_tcp.elapsed().as_secs_f64() * 1000.0;
    let _ = tcp.set_nodelay(true);
    // Dup BEFORE the stream is consumed by rustls/hyper — the read-only dup
    // keeps the kernel socket queryable after the transfer (see SocketProbe).
    let socket_probe = SocketProbe::new(&tcp);

    if is_https {
        let tls_config = build_tls_config(
            &Protocol::Http2,
            target.insecure,
            target.ca_bundle.as_deref(),
        )
        .map_err(|e| e.to_string())?;
        let connector = TlsConnector::from(Arc::new(tls_config));
        let server_name =
            ServerName::try_from(host.clone()).map_err(|e| format!("Invalid SNI: {e}"))?;
        let t_tls = Instant::now();
        let tls = tokio::time::timeout(timeout, connector.connect(server_name, tcp))
            .await
            .map_err(|_| format!("TLS handshake timed out after {}ms", target.timeout_ms))?
            .map_err(|e| format!("TLS handshake failed: {e}"))?;
        let tls_ms = t_tls.elapsed().as_secs_f64() * 1000.0;
        let (sender, conn) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
            .adaptive_window(true)
            .handshake::<_, ProbeBody>(TokioIo::new(tls))
            .await
            .map_err(|e| format!("H2 handshake failed: {e}"))?;
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                debug!("load-gen H2 connection ended: {e}");
            }
        });
        Ok(H2Connection {
            sender,
            tcp_ms,
            tls_ms: Some(tls_ms),
            socket_probe,
        })
    } else {
        // Cleartext: HTTP/2 with prior knowledge (h2c). The endpoint's plain
        // listener (hyper auto builder) detects the H2 preface.
        let (sender, conn) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
            .adaptive_window(true)
            .handshake::<_, ProbeBody>(TokioIo::new(tcp))
            .await
            .map_err(|e| format!("h2c handshake failed: {e}"))?;
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                debug!("load-gen h2c connection ended: {e}");
            }
        });
        Ok(H2Connection {
            sender,
            tcp_ms,
            tls_ms: None,
            socket_probe,
        })
    }
}

/// One counted download transfer: GET `/download?bytes=N`, adding every
/// received body byte to `bytes`.
pub async fn load_download_once(
    target: &H2Target,
    mut sender: H2Sender,
    bytes: &AtomicU64,
    request_bytes: usize,
) -> Result<(), String> {
    let host = host_header(&target.base_url);
    let req = Request::builder()
        .method("GET")
        .uri(format!("/download?bytes={request_bytes}"))
        .header("host", &host)
        .header("user-agent", "networker-tester/load-gen")
        .body(empty_body())
        .map_err(|e| e.to_string())?;
    let resp = sender.send_request(req).await.map_err(|e| e.to_string())?;
    let mut body = resp.into_body();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| e.to_string())?;
        if let Some(data) = frame.data_ref() {
            bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
        }
    }
    Ok(())
}

/// One counted upload transfer: POST `/upload` with a [`CountedUploadBody`]
/// so bytes are counted as the H2 layer accepts them under flow control.
pub async fn load_upload_once(
    target: &H2Target,
    mut sender: H2Sender,
    bytes: &Arc<AtomicU64>,
    request_bytes: usize,
) -> Result<(), String> {
    let host = host_header(&target.base_url);
    let body = CountedUploadBody::new(request_bytes, bytes.clone());
    let req = Request::builder()
        .method("POST")
        .uri("/upload")
        .header("host", &host)
        .header("user-agent", "networker-tester/load-gen")
        .header("content-length", request_bytes.to_string())
        .body(BoxBody::new(body))
        .map_err(|e| e.to_string())?;
    let resp = sender.send_request(req).await.map_err(|e| e.to_string())?;
    // Drain the (tiny JSON) response so the stream completes cleanly.
    let mut body = resp.into_body();
    while let Some(frame) = body.frame().await {
        frame.map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Upload body yielding zero-filled 64 KiB chunks, counting bytes as the H2
/// layer polls them — the poll is back-pressured by H2 flow control, so the
/// counter tracks what actually entered the send window (not what we wished
/// we could send).
pub struct CountedUploadBody {
    remaining: usize,
    chunk: Bytes,
    counter: Arc<AtomicU64>,
}

impl CountedUploadBody {
    pub fn new(total: usize, counter: Arc<AtomicU64>) -> Self {
        Self {
            remaining: total,
            chunk: Bytes::from(vec![0u8; 64 * 1024]),
            counter,
        }
    }
}

impl HyperBody for CountedUploadBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        if self.remaining == 0 {
            return Poll::Ready(None);
        }
        let n = self.remaining.min(self.chunk.len());
        let data = self.chunk.slice(..n);
        self.remaining -= n;
        self.counter.fetch_add(n as u64, Ordering::Relaxed);
        Poll::Ready(Some(Ok(hyper::body::Frame::data(data))))
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        hyper::body::SizeHint::with_exact(self.remaining as u64)
    }
}

pub fn empty_body() -> ProbeBody {
    BoxBody::new(Full::new(Bytes::new()).map_err(|never| match never {}))
}

pub fn host_header(url: &url::Url) -> String {
    let host = url.host_str().unwrap_or("localhost");
    match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Goodput-stability math (shared by both load-ramp modes)
// ─────────────────────────────────────────────────────────────────────────────

pub fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

pub fn stddev(v: &[f64]) -> f64 {
    if v.len() < 2 {
        return 0.0;
    }
    let m = mean(v);
    (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stddev_stability_criterion() {
        // Perfectly flat window → stddev 0 < 5% of anything positive.
        let flat = vec![100.0, 100.0, 100.0, 100.0];
        assert!(stddev(&flat) < 0.05 * mean(&flat));
        // Ramp still growing → not stable at 5%.
        let ramp = vec![100.0, 150.0, 200.0, 250.0];
        assert!(stddev(&ramp) >= 0.05 * mean(&ramp));
    }

    #[test]
    fn counted_upload_body_reports_exact_size_hint() {
        let counter = Arc::new(AtomicU64::new(0));
        let body = CountedUploadBody::new(1_000_000, counter);
        assert_eq!(body.size_hint().exact(), Some(1_000_000));
    }

    #[test]
    fn host_header_includes_port_when_present() {
        let with_port = url::Url::parse("http://example.com:8080/").unwrap();
        assert_eq!(host_header(&with_port), "example.com:8080");
        let no_port = url::Url::parse("https://example.com/").unwrap();
        assert_eq!(host_header(&no_port), "example.com");
    }
}
