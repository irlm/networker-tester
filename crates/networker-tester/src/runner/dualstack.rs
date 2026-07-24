//! IPv4-vs-IPv6 comparison probe (`dualstack` mode).
//!
//! Resolves A and AAAA separately (reusing the dns-depth machinery from
//! [`crate::runner::dns::resolve_detailed`]), then runs one HTTP/1.1 GET
//! pinned to each address family (reusing [`crate::runner::http::run_probe`]
//! with the `ipv4_only`/`ipv6_only` switches) and compares per-phase timing:
//! DNS, TCP connect, TLS handshake, TTFB, and total.
//!
//! - One working family = probe SUCCESS; the other family is reported as
//!   absent (no records → `attempted: false`) or failed (`error` populated),
//!   never as a fake datapoint.
//! - `faster_family`/`delta_ms` compare total_ms when both legs succeeded.
//! - The happy-eyeballs verdict follows RFC 8305's connection race: IPv6 is
//!   preferred and IPv4 only wins when IPv6's TCP connect is more than the
//!   250 ms grace period slower.

use crate::metrics::{
    DualStackLeg, DualStackResult, ErrorCategory, ErrorRecord, Protocol, RequestAttempt,
};
use crate::runner::dns::resolve_detailed;
use crate::runner::http::{run_probe, RunConfig};
use chrono::Utc;
use std::net::IpAddr;
use uuid::Uuid;

/// RFC 8305 §5 recommended connection-attempt delay.
pub const HAPPY_EYEBALLS_GRACE_MS: f64 = 250.0;

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

pub async fn run_dualstack_probe(
    run_id: Uuid,
    sequence_num: u32,
    target: &url::Url,
    cfg: &RunConfig,
) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();

    let Some(host) = target.host_str().map(str::to_string) else {
        return dualstack_failed(
            run_id,
            attempt_id,
            sequence_num,
            started_at,
            ErrorCategory::Config,
            "Target URL has no host".into(),
        );
    };
    // url::Url brackets IPv6 hosts; strip for IP-literal detection.
    let bare_host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();

    // ── Which families have addresses? ───────────────────────────────────────
    // IP literals pin the family outright; hostnames get separately-timed
    // A/AAAA lookups (dns-depth machinery).
    let (has_v4, has_v6) = match bare_host.parse::<IpAddr>() {
        Ok(IpAddr::V4(_)) => (true, false),
        Ok(IpAddr::V6(_)) => (false, true),
        Err(_) => match resolve_detailed(&bare_host, false, false).await {
            Ok((ips, _)) => (
                ips.iter().any(|ip| ip.is_ipv4()),
                ips.iter().any(|ip| ip.is_ipv6()),
            ),
            Err(e) => {
                return dualstack_failed(
                    run_id,
                    attempt_id,
                    sequence_num,
                    started_at,
                    e.category,
                    format!("dualstack resolution failed: {}", e.message),
                );
            }
        },
    };

    // ── Run one pinned HTTP/1.1 GET per available family (sequentially, so
    //    the legs don't contend for bandwidth/CPU) ─────────────────────────────
    let ipv4 = if has_v4 {
        let mut leg_cfg = cfg.clone();
        leg_cfg.ipv4_only = true;
        leg_cfg.ipv6_only = false;
        run_leg(run_id, sequence_num, target, &leg_cfg).await
    } else {
        absent_leg("no A records")
    };
    let ipv6 = if has_v6 {
        let mut leg_cfg = cfg.clone();
        leg_cfg.ipv4_only = false;
        leg_cfg.ipv6_only = true;
        run_leg(run_id, sequence_num, target, &leg_cfg).await
    } else {
        absent_leg("no AAAA records")
    };

    // ── Compare ──────────────────────────────────────────────────────────────
    let (faster_family, delta_ms) = match (ipv4.total_ms, ipv6.total_ms) {
        (Some(v4), Some(v6)) => {
            let faster = if v4 <= v6 { "ipv4" } else { "ipv6" };
            (Some(faster.to_string()), Some((v4 - v6).abs()))
        }
        _ => (None, None),
    };
    let happy_eyeballs_verdict = happy_eyeballs_verdict(&ipv4, &ipv6);

    let any_success = ipv4.success || ipv6.success;
    let error = if any_success {
        None
    } else {
        Some(ErrorRecord {
            category: ErrorCategory::Http,
            message: format!(
                "Both address families failed — ipv4: {}; ipv6: {}",
                ipv4.error.as_deref().unwrap_or("not attempted"),
                ipv6.error.as_deref().unwrap_or("not attempted"),
            ),
            detail: None,
            occurred_at: Utc::now(),
        })
    };

    let result = DualStackResult {
        ipv4,
        ipv6,
        faster_family,
        delta_ms,
        happy_eyeballs_verdict,
        happy_eyeballs_grace_ms: HAPPY_EYEBALLS_GRACE_MS,
        started_at,
    };

    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::DualStack,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        success: any_success,
        dns: None,
        tcp: None,
        tls: None,
        http: None,
        udp: None,
        error,
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
        browser: None,
        http_stack: None,
        rpm: None,
        ping: None,
        path: None,
        dualstack: Some(result),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Run one family-pinned HTTP/1.1 probe and fold its phases into a leg.
async fn run_leg(
    run_id: Uuid,
    sequence_num: u32,
    target: &url::Url,
    cfg: &RunConfig,
) -> DualStackLeg {
    let attempt = run_probe(run_id, sequence_num, Protocol::Http1, target, cfg).await;
    DualStackLeg {
        attempted: true,
        success: attempt.success,
        addr: attempt
            .dns
            .as_ref()
            .and_then(|d| d.resolved_ips.first().cloned())
            .or_else(|| {
                // dns_enabled=false path: the host itself is the address.
                target
                    .host_str()
                    .map(|h| h.trim_start_matches('[').trim_end_matches(']').to_string())
            }),
        dns_ms: attempt.dns.as_ref().map(|d| d.duration_ms),
        tcp_ms: attempt.tcp.as_ref().map(|t| t.connect_duration_ms),
        tls_ms: attempt.tls.as_ref().map(|t| t.handshake_duration_ms),
        ttfb_ms: attempt.http.as_ref().map(|h| h.ttfb_ms),
        // total_ms only for completed requests: a leg that died mid-phase has
        // no comparable end-to-end number.
        total_ms: attempt
            .http
            .as_ref()
            .filter(|_| attempt.success)
            .map(|h| h.total_duration_ms),
        error: attempt.error.map(|e| e.message),
    }
}

fn absent_leg(reason: &str) -> DualStackLeg {
    DualStackLeg {
        attempted: false,
        success: false,
        addr: None,
        dns_ms: None,
        tcp_ms: None,
        tls_ms: None,
        ttfb_ms: None,
        total_ms: None,
        error: Some(reason.to_string()),
    }
}

/// RFC 8305 connection race: IPv6 starts first; IPv4 starts after the grace
/// period. IPv6 wins unless its connect is more than the grace slower than
/// IPv4's (or it failed outright).
fn happy_eyeballs_verdict(ipv4: &DualStackLeg, ipv6: &DualStackLeg) -> String {
    match (ipv4.success, ipv6.success) {
        (false, false) => "none (both families failed)".to_string(),
        (true, false) => {
            if ipv6.attempted {
                "ipv4 (ipv6 attempted but failed)".to_string()
            } else {
                "ipv4 (only family with records)".to_string()
            }
        }
        (false, true) => {
            if ipv4.attempted {
                "ipv6 (ipv4 attempted but failed)".to_string()
            } else {
                "ipv6 (only family with records)".to_string()
            }
        }
        (true, true) => {
            // Compare connection-establishment time — the phase the RFC 8305
            // race actually staggers. Fall back to totals when a leg somehow
            // lacks a TCP phase.
            let v4 = ipv4.tcp_ms.or(ipv4.total_ms).unwrap_or(f64::MAX);
            let v6 = ipv6.tcp_ms.or(ipv6.total_ms).unwrap_or(f64::MAX);
            if v6 <= v4 + HAPPY_EYEBALLS_GRACE_MS {
                format!(
                    "ipv6 (connect {v6:.1}ms within {HAPPY_EYEBALLS_GRACE_MS:.0}ms grace of ipv4's {v4:.1}ms)"
                )
            } else {
                format!(
                    "ipv4 (ipv6 connect {v6:.1}ms exceeds ipv4's {v4:.1}ms by more than the {HAPPY_EYEBALLS_GRACE_MS:.0}ms grace)"
                )
            }
        }
    }
}

fn dualstack_failed(
    run_id: Uuid,
    attempt_id: Uuid,
    sequence_num: u32,
    started_at: chrono::DateTime<Utc>,
    category: ErrorCategory,
    message: String,
) -> RequestAttempt {
    RequestAttempt {
        attempt_id,
        run_id,
        protocol: Protocol::DualStack,
        sequence_num,
        started_at,
        finished_at: Some(Utc::now()),
        success: false,
        dns: None,
        tcp: None,
        tls: None,
        http: None,
        udp: None,
        error: Some(ErrorRecord {
            category,
            message,
            detail: None,
            occurred_at: Utc::now(),
        }),
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
        browser: None,
        http_stack: None,
        rpm: None,
        ping: None,
        path: None,
        dualstack: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn leg(success: bool, tcp_ms: Option<f64>, total_ms: Option<f64>) -> DualStackLeg {
        DualStackLeg {
            attempted: true,
            success,
            addr: None,
            dns_ms: None,
            tcp_ms,
            tls_ms: None,
            ttfb_ms: None,
            total_ms,
            error: None,
        }
    }

    #[test]
    fn verdict_prefers_ipv6_within_grace() {
        let v = happy_eyeballs_verdict(
            &leg(true, Some(10.0), Some(50.0)),
            &leg(true, Some(200.0), Some(240.0)),
        );
        assert!(v.starts_with("ipv6"), "{v}");
    }

    #[test]
    fn verdict_falls_back_to_ipv4_beyond_grace() {
        let v = happy_eyeballs_verdict(
            &leg(true, Some(10.0), Some(50.0)),
            &leg(true, Some(300.0), Some(340.0)),
        );
        assert!(v.starts_with("ipv4"), "{v}");
    }

    #[test]
    fn verdict_single_family() {
        let v = happy_eyeballs_verdict(&leg(true, Some(5.0), Some(20.0)), &absent_leg("no AAAA"));
        assert_eq!(v, "ipv4 (only family with records)");
        let v = happy_eyeballs_verdict(&absent_leg("no A"), &leg(true, Some(5.0), Some(20.0)));
        assert_eq!(v, "ipv6 (only family with records)");
    }

    #[test]
    fn verdict_failed_family_is_called_out() {
        let mut v6 = leg(false, None, None);
        v6.error = Some("connect refused".into());
        let v = happy_eyeballs_verdict(&leg(true, Some(5.0), Some(20.0)), &v6);
        assert_eq!(v, "ipv4 (ipv6 attempted but failed)");
    }

    #[test]
    fn absent_leg_is_honest() {
        let l = absent_leg("no AAAA records");
        assert!(!l.attempted);
        assert!(!l.success);
        assert!(l.total_ms.is_none());
        assert_eq!(l.error.as_deref(), Some("no AAAA records"));
    }
}
