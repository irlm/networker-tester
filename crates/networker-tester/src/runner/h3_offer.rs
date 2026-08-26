//! HTTP/3 pre-flight: does this target actually offer h3?
//!
//! A raw URL's HTTP/3 support cannot be known statically. The control plane's
//! config-create gate ([`ModeTargetCompatibility`] on the C# side) can only
//! decide it for LagHound-managed targets, where the proxy stack is known from
//! `shared/http-stacks.json`; for an arbitrary third-party URL it has nothing
//! to go on, so the h3 modes are dispatched and fail.
//!
//! Over the web, an origin advertises HTTP/3 with `Alt-Svc: h3=":443"` (RFC
//! 9114 §3.1). Without that advertisement a normal client has no way to
//! discover h3, so "responded, advertised no h3" is a sound reading of "this
//! target does not offer HTTP/3". That is the same signal `install.sh` already
//! uses to decide whether a stack came up with QUIC (`curl -I | grep
//! 'alt-svc.*h3'`), and every h3-capable stack the installer configures sets
//! the header explicitly (nginx :8444, caddy :8454, IIS :8445, and the bare
//! networker-endpoint).
//!
//! **Fail open, always.** Only a response that we actually read and that
//! carried no h3 token gates a probe. A timeout, a connection error, a
//! non-response — anything that leaves us uncertain — runs the probe as
//! before, because a pre-flight must never invent a failure the network did
//! not produce.
//!
//! **Known gap:** an origin may also publish h3 through a DNS HTTPS/SVCB
//! record (RFC 9460) with no `Alt-Svc` header. Such a target is read as
//! not-offered here. It is rare in practice, and the cost is the pre-#0.28.301
//! behaviour (the h3 probe runs and fails), not a wrong measurement.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use uuid::Uuid;

use crate::metrics::Protocol;
use crate::runner::http::RunConfig;

/// What the target said about HTTP/3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum H3Offer {
    /// The origin advertised h3; the payload is the raw `Alt-Svc` value.
    Advertised(String),
    /// The origin responded and advertised no h3.
    NotOffered,
    /// We could not find out. Callers MUST fail open on this.
    Unknown,
}

/// Does an `Alt-Svc` field value advertise an HTTP/3 protocol id?
///
/// Accepts the registered `h3` token and the draft ids still seen in the wild
/// (`h3-29`, `h3-Q050`, …), and ignores a `clear` advertisement, which
/// withdraws every alternative rather than offering one.
pub fn advertises_h3(alt_svc: &str) -> bool {
    for entry in alt_svc.split(',') {
        // Each entry is `protocol-id="host:port"; param=...`; the protocol id
        // is everything before the first '='.
        let Some((id, _)) = entry.split_once('=') else {
            continue;
        };
        let id = id.trim().trim_matches('"').to_ascii_lowercase();
        if id == "clear" {
            continue;
        }
        if id == "h3" || id.starts_with("h3-") {
            return true;
        }
    }
    false
}

/// The `Alt-Svc` value from a probe's captured response headers, if any.
fn alt_svc_of(headers: &[(String, String)]) -> Option<&str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("alt-svc"))
        .map(|(_, v)| v.as_str())
}

/// Origin key for the memo: scheme + host + port, ignoring path and query.
fn origin_of(target: &url::Url) -> String {
    match (target.host_str(), target.port_or_known_default()) {
        (Some(h), Some(p)) => format!("{}://{h}:{p}", target.scheme()),
        (Some(h), None) => format!("{}://{h}", target.scheme()),
        _ => target.as_str().to_string(),
    }
}

fn memo() -> &'static Mutex<HashMap<String, H3Offer>> {
    static MEMO: OnceLock<Mutex<HashMap<String, H3Offer>>> = OnceLock::new();
    MEMO.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Ask the target whether it offers HTTP/3, memoised per origin for the life of
/// the process (one run = one tester invocation, so this costs at most one
/// extra HTTP/1.1 request per origin no matter how many h3 samples follow).
///
/// The probe reuses [`crate::runner::http::run_probe`], so it honours the run's
/// timeout, `--insecure` and CA bundle exactly as every other request does.
pub async fn discover(run_id: Uuid, target: &url::Url, cfg: &RunConfig) -> H3Offer {
    let key = origin_of(target);
    if let Ok(m) = memo().lock() {
        if let Some(hit) = m.get(&key) {
            return hit.clone();
        }
    }

    let offer = probe(run_id, target, cfg).await;

    if let Ok(mut m) = memo().lock() {
        m.insert(key, offer.clone());
    }
    offer
}

async fn probe(run_id: Uuid, target: &url::Url, cfg: &RunConfig) -> H3Offer {
    // HTTP/1.1: the most broadly accepted version, and `Alt-Svc` is served
    // independently of the version that carried the response.
    let attempt = crate::runner::http::run_probe(run_id, 0, Protocol::Http1, target, cfg).await;

    let Some(http) = attempt.http.as_ref() else {
        return H3Offer::Unknown;
    };
    if !attempt.success {
        // The origin did not give us a clean answer — say nothing.
        return H3Offer::Unknown;
    }

    match alt_svc_of(&http.response_headers) {
        Some(v) if advertises_h3(v) => H3Offer::Advertised(v.to_string()),
        Some(_) | None => H3Offer::NotOffered,
    }
}

/// Test seam: forget everything learned so far.
#[cfg(test)]
pub fn reset_memo_for_tests() {
    if let Ok(mut m) = memo().lock() {
        m.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_the_registered_h3_token() {
        assert!(advertises_h3(r#"h3=":443"; ma=86400"#));
        assert!(advertises_h3(r#"h3=":8444"; ma=86400"#));
    }

    #[test]
    fn detects_draft_h3_tokens_alongside_others() {
        assert!(advertises_h3(r#"h3-29=":443"; ma=2592000,h2=":443""#));
        assert!(advertises_h3(r#"h2=":443", h3=":443""#));
    }

    #[test]
    fn rejects_an_advert_without_h3() {
        // www.microsoft.com serves no Alt-Svc at all; a target that advertises
        // only h2 is the same answer for our purposes.
        assert!(!advertises_h3(r#"h2=":443"; ma=86400"#));
        assert!(!advertises_h3(""));
        assert!(!advertises_h3("clear"));
    }

    #[test]
    fn does_not_confuse_h3_inside_another_token() {
        // A parameter or host that merely contains "h3" is not an advert.
        assert!(!advertises_h3(r#"h2=":443"; persist=h3"#));
        assert!(!advertises_h3(r#"noth3=":443""#));
    }

    #[test]
    fn alt_svc_lookup_is_case_insensitive() {
        let headers = vec![("Alt-Svc".to_string(), r#"h3=":443""#.to_string())];
        assert_eq!(alt_svc_of(&headers), Some(r#"h3=":443""#));
    }

    #[test]
    fn origin_key_ignores_path_and_query() {
        let a = url::Url::parse("https://example.com/one?x=1").unwrap();
        let b = url::Url::parse("https://example.com/two").unwrap();
        assert_eq!(origin_of(&a), origin_of(&b));
        assert_eq!(origin_of(&a), "https://example.com:443");
    }

    #[test]
    fn origin_key_separates_ports_and_schemes() {
        let https = url::Url::parse("https://example.com/").unwrap();
        let http = url::Url::parse("http://example.com/").unwrap();
        let alt = url::Url::parse("https://example.com:8444/").unwrap();
        assert_ne!(origin_of(&https), origin_of(&http));
        assert_ne!(origin_of(&https), origin_of(&alt));
    }
}
