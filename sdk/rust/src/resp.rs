//! The unified LagHound response body and response helpers (contract §3, §7).

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::{HeaderValue, Response, StatusCode};
use http_body::{Body, Frame, SizeHint};
use serde_json::Value;

use crate::body::DownloadBody;
use crate::limits::Permit;
use crate::timing::{build_header, Metric};

/// LagHound's own response body: either a single in-memory buffer (JSON / empty)
/// or the streamed download body. Small enum, no trait objects.
///
/// An optional [`Permit`] rides along and is dropped when the body is dropped —
/// this is how a transfer holds its concurrency slot for the whole transfer,
/// not just handler setup (contract §6.3).
pub struct LagBody {
    kind: LagBodyKind,
    _permit: Option<Permit>,
}

enum LagBodyKind {
    Full(Option<Bytes>),
    Download(DownloadBody),
}

impl LagBody {
    pub fn full(bytes: Bytes) -> Self {
        LagBody {
            kind: LagBodyKind::Full(Some(bytes)),
            _permit: None,
        }
    }
    pub fn empty() -> Self {
        LagBody {
            kind: LagBodyKind::Full(None),
            _permit: None,
        }
    }
    /// Attach a concurrency permit whose lifetime is bound to this body.
    pub fn with_permit(mut self, permit: Option<Permit>) -> Self {
        self._permit = permit;
        self
    }
}

impl Body for LagBody {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match &mut self.get_mut().kind {
            LagBodyKind::Full(opt) => match opt.take() {
                Some(b) if !b.is_empty() => Poll::Ready(Some(Ok(Frame::data(b)))),
                _ => Poll::Ready(None),
            },
            LagBodyKind::Download(d) => Pin::new(d).poll_frame(cx),
        }
    }

    fn is_end_stream(&self) -> bool {
        match &self.kind {
            LagBodyKind::Full(opt) => opt.is_none(),
            LagBodyKind::Download(d) => d.is_end_stream(),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match &self.kind {
            LagBodyKind::Full(Some(b)) => SizeHint::with_exact(b.len() as u64),
            LagBodyKind::Full(None) => SizeHint::with_exact(0),
            LagBodyKind::Download(d) => d.size_hint(),
        }
    }
}

const CACHE_CONTROL: &str = "no-store, no-cache, must-revalidate";

/// A bare 404 (contract §5, §6.5): no body, no LagHound headers, no envelope,
/// no `Server-Timing`, no `WWW-Authenticate`.
pub fn bare_404() -> Response<LagBody> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(LagBody::empty())
        .expect("static 404 response is valid")
}

/// Attach the common success headers (contract §3): `Server-Timing`,
/// `Cache-Control`, and `Timing-Allow-Origin` (contract §4.4).
fn common_headers(builder: http::response::Builder, timing: &[Metric]) -> http::response::Builder {
    let mut b = builder
        .header(http::header::CACHE_CONTROL, CACHE_CONTROL)
        .header("timing-allow-origin", "*");
    let header = build_header(timing);
    if let Ok(v) = HeaderValue::from_str(&header) {
        b = b.header("server-timing", v);
    }
    b
}

/// A JSON response with the common LagHound headers and a `Server-Timing` list.
pub fn json_response(
    status: StatusCode,
    value: &Value,
    timing: &[Metric],
    extra: &[(&str, String)],
) -> Response<LagBody> {
    let body = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    let mut builder = common_headers(Response::builder().status(status), timing)
        .header(http::header::CONTENT_TYPE, "application/json");
    for (k, v) in extra {
        if let Ok(val) = HeaderValue::from_str(v) {
            builder = builder.header(*k, val);
        }
    }
    builder
        .header(http::header::CONTENT_LENGTH, body.len())
        .body(LagBody::full(Bytes::from(body)))
        .expect("json response is valid")
}

/// The download response: octet-stream, streamed body, `Content-Length` and
/// `X-LagHound-Bytes` set to the clamped actual size (contract §3.3). The
/// `permit` (if any) rides in the body so the transfer slot is held until the
/// body is fully sent/dropped.
pub fn download_response(
    body: DownloadBody,
    actual: u64,
    timing: &[Metric],
    permit: Option<Permit>,
) -> Response<LagBody> {
    let lag = LagBody {
        kind: LagBodyKind::Download(body),
        _permit: permit,
    };
    common_headers(Response::builder().status(StatusCode::OK), timing)
        .header(http::header::CONTENT_TYPE, "application/octet-stream")
        .header(http::header::CONTENT_LENGTH, actual)
        .header("x-laghound-bytes", actual)
        .body(lag)
        .expect("download response is valid")
}
