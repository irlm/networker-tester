use crate::metrics::{DnsResult, ErrorCategory, ErrorRecord, Protocol, RequestAttempt};
use chrono::Utc;
use hickory_resolver::{
    config::{ResolverConfig, ResolverOpts},
    TokioAsyncResolver,
};
use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::Instant;
use tracing::warn;
use uuid::Uuid;

/// Process-wide resolver plus a human-readable identity label.
///
/// Uses the OS resolver configuration (/etc/resolv.conf, macOS SystemConfiguration,
/// Windows registry) so DNS timing measures the path the user's applications
/// actually take. Falls back to Google Public DNS only when the system config
/// cannot be read — and says so in the label. (Trust audit V1: the resolver was
/// previously hardcoded to 8.8.8.8 via `ResolverConfig::default()`.)
///
/// Built once per process: reading the system config touches the filesystem and
/// must never pollute a DNS timing window.
fn shared_resolver() -> &'static (TokioAsyncResolver, String) {
    static RESOLVER: OnceLock<(TokioAsyncResolver, String)> = OnceLock::new();
    RESOLVER.get_or_init(|| match hickory_resolver::system_conf::read_system_conf() {
        Ok((config, opts)) => {
            let mut servers: Vec<String> = config
                .name_servers()
                .iter()
                .map(|ns| ns.socket_addr.to_string())
                .collect();
            servers.dedup(); // UDP+TCP entries share a socket addr and are adjacent
            let label = format!("system ({})", servers.join(", "));
            (TokioAsyncResolver::tokio(config, opts), label)
        }
        Err(e) => {
            warn!(
                "Cannot read system resolver config ({e}); \
                 falling back to Google Public DNS — DNS timings will NOT \
                 reflect the path your applications use"
            );
            (
                TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default()),
                "google-fallback (8.8.8.8:53) — system config unavailable".to_string(),
            )
        }
    })
}

/// Resolve `hostname` and return the list of IPs plus a timing record.
/// Respects `ipv4_only` / `ipv6_only` filtering.
pub async fn resolve(
    hostname: &str,
    ipv4_only: bool,
    ipv6_only: bool,
) -> Result<(Vec<IpAddr>, DnsResult), ErrorRecord> {
    // Obtain the (cached) resolver before starting the timer: resolver
    // construction reads config files and is not part of DNS resolution time.
    let (resolver, resolver_label) = shared_resolver();

    let started_at = Utc::now();
    let t0 = Instant::now();

    let lookup = resolver
        .lookup_ip(hostname)
        .await
        .map_err(|e| ErrorRecord {
            category: ErrorCategory::Dns,
            message: e.to_string(),
            detail: Some(format!("lookup_ip({hostname}) failed")),
            occurred_at: Utc::now(),
        })?;

    let mut ips: Vec<IpAddr> = lookup.iter().collect();

    if ipv4_only {
        ips.retain(|ip| ip.is_ipv4());
    } else if ipv6_only {
        ips.retain(|ip| ip.is_ipv6());
    }

    let duration_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let result = DnsResult {
        query_name: hostname.to_string(),
        resolved_ips: ips.iter().map(IpAddr::to_string).collect(),
        duration_ms,
        started_at,
        success: !ips.is_empty(),
        resolver: Some(resolver_label.clone()),
    };

    if ips.is_empty() {
        return Err(ErrorRecord {
            category: ErrorCategory::Dns,
            message: format!("No IPs resolved for {hostname} (filter: ipv4_only={ipv4_only}, ipv6_only={ipv6_only})"),
            detail: None,
            occurred_at: Utc::now(),
        });
    }

    Ok((ips, result))
}

/// Standalone DNS probe: resolves the target hostname and returns a complete
/// `RequestAttempt` with only a `DnsResult` populated (no TCP/TLS/HTTP).
pub async fn run_dns_probe(
    run_id: Uuid,
    sequence_num: u32,
    hostname: &str,
    ipv4_only: bool,
    ipv6_only: bool,
) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();

    match resolve(hostname, ipv4_only, ipv6_only).await {
        Ok((_, dns_result)) => RequestAttempt {
            attempt_id,
            run_id,
            protocol: Protocol::Dns,
            sequence_num,
            started_at,
            finished_at: Some(Utc::now()),
            success: dns_result.success,
            dns: Some(dns_result),
            tcp: None,
            tls: None,
            http: None,
            udp: None,
            error: None,
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: None,
        },
        Err(err) => RequestAttempt {
            attempt_id,
            run_id,
            protocol: Protocol::Dns,
            sequence_num,
            started_at,
            finished_at: Some(Utc::now()),
            success: false,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: None,
            error: Some(err),
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolves "localhost" — loopback always resolves in any environment.
    #[tokio::test]
    async fn resolves_localhost() {
        let (ips, r) = resolve("localhost", false, false).await.unwrap();
        assert!(!ips.is_empty());
        assert!(r.success);
        assert!(r.duration_ms >= 0.0);
    }

    /// Regression test for trust-audit V1: the resolver identity must be
    /// recorded, and on a machine with a readable system config it must be the
    /// system resolver — not the old hardcoded Google Public DNS default.
    #[tokio::test]
    async fn records_resolver_identity() {
        let (_, r) = resolve("localhost", false, false).await.unwrap();
        let resolver = r.resolver.expect("resolver identity must be recorded");
        assert!(!resolver.is_empty());
        if resolver.starts_with("system") {
            // System path: label lists actual nameserver socket addrs.
            assert!(
                resolver.contains(':'),
                "expected socket addrs in {resolver}"
            );
        } else {
            // Fallback path must be loudly labeled as such.
            assert!(
                resolver.contains("fallback"),
                "non-system resolver must be labeled a fallback, got: {resolver}"
            );
        }
    }

    #[tokio::test]
    async fn ipv4_only_filter() {
        let (ips, _) = resolve("localhost", true, false).await.unwrap();
        assert!(ips.iter().all(|ip| ip.is_ipv4()));
    }

    #[tokio::test]
    async fn ipv6_only_filter_on_localhost() {
        // "localhost" may or may not have AAAA records depending on the OS.
        // We just verify the filter is applied — if it returns Ok, all IPs are v6;
        // if it returns Err (no IPv6 for localhost), that's also valid.
        if let Ok((ips, _)) = resolve("localhost", false, true).await {
            assert!(ips.iter().all(|ip| ip.is_ipv6()));
        }
        // Err = no IPv6 address for localhost — filter worked correctly
    }

    #[tokio::test]
    async fn unresolvable_hostname_returns_error() {
        let result = resolve("this-hostname-does-not-exist.invalid", false, false).await;
        assert!(result.is_err(), "should fail for an unresolvable hostname");
    }

    #[tokio::test]
    async fn run_dns_probe_success_populates_dns_result() {
        let run_id = uuid::Uuid::new_v4();
        let attempt = run_dns_probe(run_id, 0, "localhost", false, false).await;
        assert!(attempt.success, "probe should succeed: {:?}", attempt.error);
        assert_eq!(attempt.protocol, Protocol::Dns);
        let dns = attempt.dns.expect("dns result should be present");
        assert!(!dns.resolved_ips.is_empty());
        assert!(attempt.tcp.is_none());
        assert!(attempt.http.is_none());
    }

    #[tokio::test]
    async fn run_dns_probe_failure_sets_error() {
        let run_id = uuid::Uuid::new_v4();
        let attempt = run_dns_probe(
            run_id,
            0,
            "this-hostname-does-not-exist.invalid",
            false,
            false,
        )
        .await;
        assert!(!attempt.success);
        assert!(attempt.error.is_some());
        assert!(attempt.dns.is_none());
    }
}
