//! Enveloped errors (contract v1 §7). Messages are fixed strings — never
//! interpolated request data.

use http::StatusCode;
use serde_json::json;

use crate::resp::{json_response, LagBody};
use crate::timing::Metric;

/// An enveloped error code (contract §7 table).
#[derive(Clone, Copy, Debug)]
pub enum ErrorCode {
    InvalidParam,
    MethodNotAllowed,
    PayloadTooLarge,
    RateLimited,
    Internal,
}

impl ErrorCode {
    pub fn status(self) -> StatusCode {
        match self {
            ErrorCode::InvalidParam => StatusCode::BAD_REQUEST,
            ErrorCode::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            ErrorCode::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            ErrorCode::InvalidParam => "invalid_param",
            ErrorCode::MethodNotAllowed => "method_not_allowed",
            ErrorCode::PayloadTooLarge => "payload_too_large",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::Internal => "internal",
        }
    }

    /// Fixed message string (contract §7 — never echoes request data).
    pub fn message(self) -> &'static str {
        match self {
            ErrorCode::InvalidParam => "invalid parameter",
            ErrorCode::MethodNotAllowed => "method not allowed",
            ErrorCode::PayloadTooLarge => "payload too large",
            ErrorCode::RateLimited => "rate limit exceeded",
            ErrorCode::Internal => "internal error",
        }
    }
}

/// Build an enveloped error response. `retry_after_ms`, when `Some`, sets both
/// the JSON field and the `Retry-After` header (seconds, rounded up).
pub fn envelope(
    code: ErrorCode,
    retry_after_ms: Option<u64>,
    timing: &[Metric],
) -> http::Response<LagBody> {
    let mut err = json!({ "code": code.code(), "message": code.message() });
    let mut extra: Vec<(&str, String)> = Vec::new();
    if let Some(ms) = retry_after_ms {
        err["retry_after_ms"] = json!(ms);
        // Retry-After is in seconds; round up so it is never 0 for a positive
        // wait (contract §6.2 requires the header present on 429).
        let secs = ms.div_ceil(1000).max(1);
        extra.push(("retry-after", secs.to_string()));
    }
    let body = json!({ "contract": "v1", "error": err });
    json_response(code.status(), &body, timing, &extra)
}
