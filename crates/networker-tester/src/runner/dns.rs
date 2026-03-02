use crate::metrics::{DnsResult, ErrorCategory, ErrorRecord, Protocol, RequestAttempt};
use chrono::Utc;
use hickory_resolver::{
    config::{ResolverConfig, ResolverOpts},
    TokioAsyncResolver,
};
use std::net::IpAddr;
use std::time::Instant;
use uuid::Uuid;

/// Resolve `hostname` and return the list of IPs plus a timing record.
/// Respects `ipv4_only` / `ipv6_only` filtering.
pub async fn resolve(
    hostname: &str,
    ipv4_only: bool,
    ipv6_only: bool,
) -> Result<(Vec<IpAddr>, DnsResult), ErrorRecord> {
    let started_at = Utc::now();
    let t0 = Instant::now();

    let resolver = TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default());

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

    #[tokio::test]
    async fn ipv4_only_filter() {
        let (ips, _) = resolve("localhost", true, false).await.unwrap();
        assert!(ips.iter().all(|ip| ip.is_ipv4()));
    }
}
