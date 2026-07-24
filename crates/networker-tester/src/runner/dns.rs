use crate::metrics::{DnsResult, ErrorCategory, ErrorRecord, Protocol, RequestAttempt};
use chrono::Utc;
use hickory_resolver::{
    config::{LookupIpStrategy, ResolverConfig, ResolverOpts, GOOGLE},
    net::runtime::TokioRuntimeProvider,
    proto::rr::{RData, Record, RecordType},
    TokioResolver,
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
///
/// hickory 0.26: construction goes through `ResolverBuilder`, whose `build()`
/// is fallible. Without TLS features the only fallible branch (TLS provider
/// setup) is compiled out, but we still surface a construction error as a DNS
/// `ErrorRecord` rather than panicking inside a probe.
fn shared_resolver() -> Result<&'static (TokioResolver, String), String> {
    static RESOLVER: OnceLock<Result<(TokioResolver, String), String>> = OnceLock::new();
    RESOLVER
        .get_or_init(|| {
            let (config, opts, label) = match hickory_resolver::system_conf::read_system_conf() {
                Ok((config, opts)) => {
                    let mut servers: Vec<String> = config
                        .name_servers()
                        .iter()
                        .map(|ns| {
                            // 0.26 splits a nameserver into ip + per-protocol
                            // connections; keep the 0.24 "ip:port" label shape
                            // (UDP and TCP share the port, so the first entry
                            // is representative).
                            match ns.connections.first() {
                                Some(conn) => format!("{}:{}", ns.ip, conn.port),
                                None => ns.ip.to_string(),
                            }
                        })
                        .collect();
                    servers.dedup(); // duplicate config entries are adjacent
                    let label = format!("system ({})", servers.join(", "));
                    (config, opts, label)
                }
                Err(e) => {
                    warn!(
                        "Cannot read system resolver config ({e}); \
                         falling back to Google Public DNS — DNS timings will NOT \
                         reflect the path your applications use"
                    );
                    (
                        ResolverConfig::udp_and_tcp(&GOOGLE),
                        ResolverOpts::default(),
                        "google-fallback (8.8.8.8:53) — system config unavailable".to_string(),
                    )
                }
            };
            let mut opts = opts;
            // hickory 0.26 changed the default lookup strategy to Ipv6AndIpv4
            // (AAAA ordered before A). Pin the 0.24 behavior — A first, AAAA
            // only if A fails — so resolved-IP ordering, and therefore which
            // address downstream TCP/TLS/HTTP probes connect to, is unchanged.
            // (No resolv.conf/system option maps to this, so overriding after
            // read_system_conf() cannot clobber user configuration.)
            opts.ip_strategy = LookupIpStrategy::Ipv4thenIpv6;
            let mut builder =
                TokioResolver::builder_with_config(config, TokioRuntimeProvider::default());
            // Apply the options exactly as hickory's own `Resolver::builder()`
            // does for the system-config path.
            *builder.options_mut() = opts;
            builder
                .build()
                .map(|resolver| (resolver, label))
                .map_err(|e| format!("failed to construct DNS resolver: {e}"))
        })
        .as_ref()
        .map_err(Clone::clone)
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
    let (resolver, resolver_label) = shared_resolver().map_err(|msg| ErrorRecord {
        category: ErrorCategory::Dns,
        message: msg,
        detail: Some("resolver construction failed before any query was sent".to_string()),
        occurred_at: Utc::now(),
    })?;

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
        a_ms: None,
        aaaa_ms: None,
        a_record_count: None,
        aaaa_record_count: None,
        cname_chain: Vec::new(),
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

/// Outcome of one timed, single-record-type lookup (dns probe mode only).
struct TypedLookup {
    duration_ms: f64,
    record_count: u32,
    ips: Vec<IpAddr>,
    cname_chain: Vec<String>,
    error: Option<String>,
}

/// Perform one timed lookup for a specific record type. Errors (including
/// NODATA/NXDOMAIN responses) are captured, not propagated: the observed
/// duration is still a valid latency for that query.
async fn timed_typed_lookup(
    resolver: &TokioResolver,
    hostname: &str,
    record_type: RecordType,
) -> TypedLookup {
    let t0 = Instant::now();
    match resolver.lookup(hostname, record_type).await {
        Ok(lookup) => {
            let duration_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let answers = lookup.answers();
            let mut ips = Vec::new();
            let mut record_count = 0u32;
            for record in answers {
                match &record.data {
                    RData::A(a) if record_type == RecordType::A => {
                        record_count += 1;
                        ips.push(IpAddr::V4(a.0));
                    }
                    RData::AAAA(aaaa) if record_type == RecordType::AAAA => {
                        record_count += 1;
                        ips.push(IpAddr::V6(aaaa.0));
                    }
                    _ => {}
                }
            }
            let cname_chain = extract_cname_chain(hostname, answers);
            TypedLookup {
                duration_ms,
                record_count,
                ips,
                cname_chain,
                error: None,
            }
        }
        Err(e) => TypedLookup {
            duration_ms: t0.elapsed().as_secs_f64() * 1000.0,
            record_count: 0,
            ips: Vec::new(),
            cname_chain: Vec::new(),
            error: Some(e.to_string()),
        },
    }
}

/// Walk the CNAME records in a DNS answer section and return the chain of
/// targets in resolution order, starting from `query_name`. Comparison is
/// case-insensitive and ignores the trailing root dot; output names are
/// normalized the same way. Bounded by the number of CNAME records in the
/// answer, so a malicious CNAME loop cannot spin forever.
fn extract_cname_chain(query_name: &str, records: &[Record]) -> Vec<String> {
    fn normalize(name: &str) -> String {
        name.trim_end_matches('.').to_ascii_lowercase()
    }
    let pairs: Vec<(String, String)> = records
        .iter()
        .filter_map(|r| match &r.data {
            RData::CNAME(cname) => {
                Some((normalize(&r.name.to_utf8()), normalize(&cname.0.to_utf8())))
            }
            _ => None,
        })
        .collect();

    let mut chain = Vec::new();
    let mut current = normalize(query_name);
    while chain.len() < pairs.len() {
        match pairs.iter().find(|(owner, _)| *owner == current) {
            Some((_, target)) => {
                if chain.contains(target) {
                    break; // CNAME loop — stop rather than repeat
                }
                chain.push(target.clone());
                current = target.clone();
            }
            None => break,
        }
    }
    chain
}

/// Detailed resolution for the standalone `dns` probe mode ONLY: performs
/// separately-timed A and AAAA lookups (skipping the family excluded by
/// `ipv4_only`/`ipv6_only`) and captures per-record-type timing, record counts,
/// and the CNAME chain. Other modes keep using [`resolve`] — a single
/// dual-stack lookup — so their resolution path is not slowed down.
pub async fn resolve_detailed(
    hostname: &str,
    ipv4_only: bool,
    ipv6_only: bool,
) -> Result<(Vec<IpAddr>, DnsResult), ErrorRecord> {
    // Cached resolver obtained before the timer, exactly like `resolve`.
    let (resolver, resolver_label) = shared_resolver().map_err(|msg| ErrorRecord {
        category: ErrorCategory::Dns,
        message: msg,
        detail: Some("resolver construction failed before any query was sent".to_string()),
        occurred_at: Utc::now(),
    })?;

    let started_at = Utc::now();
    let t0 = Instant::now();

    let a = if ipv6_only {
        None
    } else {
        Some(timed_typed_lookup(resolver, hostname, RecordType::A).await)
    };
    let aaaa = if ipv4_only {
        None
    } else {
        Some(timed_typed_lookup(resolver, hostname, RecordType::AAAA).await)
    };

    let duration_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // Hard failure only when every performed lookup errored (a missing AAAA on
    // an IPv4-only host is a NODATA answer, not a probe failure).
    let performed: Vec<&TypedLookup> = a.iter().chain(aaaa.iter()).collect();
    if performed.iter().all(|l| l.error.is_some()) {
        let message = performed
            .iter()
            .filter_map(|l| l.error.clone())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(ErrorRecord {
            category: ErrorCategory::Dns,
            message,
            detail: Some(format!("A/AAAA lookups for {hostname} failed")),
            occurred_at: Utc::now(),
        });
    }

    let mut ips: Vec<IpAddr> = Vec::new();
    if let Some(l) = &a {
        ips.extend(&l.ips);
    }
    if let Some(l) = &aaaa {
        ips.extend(&l.ips);
    }

    // The chain is the same regardless of address family; prefer the A answer,
    // fall back to AAAA (e.g. --ipv6-only or IPv6-only hosts).
    let cname_chain = a
        .as_ref()
        .map(|l| l.cname_chain.clone())
        .filter(|c| !c.is_empty())
        .or_else(|| aaaa.as_ref().map(|l| l.cname_chain.clone()))
        .unwrap_or_default();

    let result = DnsResult {
        query_name: hostname.to_string(),
        resolved_ips: ips.iter().map(IpAddr::to_string).collect(),
        duration_ms,
        started_at,
        success: !ips.is_empty(),
        resolver: Some(resolver_label.clone()),
        a_ms: a.as_ref().map(|l| l.duration_ms),
        aaaa_ms: aaaa.as_ref().map(|l| l.duration_ms),
        a_record_count: a.as_ref().map(|l| l.record_count),
        aaaa_record_count: aaaa.as_ref().map(|l| l.record_count),
        cname_chain,
    };

    if ips.is_empty() {
        return Err(ErrorRecord {
            category: ErrorCategory::Dns,
            message: format!(
                "No IPs resolved for {hostname} (filter: ipv4_only={ipv4_only}, ipv6_only={ipv6_only})"
            ),
            detail: None,
            occurred_at: Utc::now(),
        });
    }

    Ok((ips, result))
}

/// Standalone DNS probe: resolves the target hostname and returns a complete
/// `RequestAttempt` with only a `DnsResult` populated (no TCP/TLS/HTTP).
/// Uses [`resolve_detailed`] for per-record-type timing and the CNAME chain.
pub async fn run_dns_probe(
    run_id: Uuid,
    sequence_num: u32,
    hostname: &str,
    ipv4_only: bool,
    ipv6_only: bool,
) -> RequestAttempt {
    let attempt_id = Uuid::new_v4();
    let started_at = Utc::now();

    match resolve_detailed(hostname, ipv4_only, ipv6_only).await {
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

    // ── extract_cname_chain ──────────────────────────────────────────────────

    use hickory_resolver::proto::rr::{rdata, Name};

    fn cname_rec(owner: &str, target: &str) -> Record {
        Record::from_rdata(
            Name::from_utf8(owner).unwrap(),
            60,
            RData::CNAME(rdata::CNAME(Name::from_utf8(target).unwrap())),
        )
    }

    fn a_rec(owner: &str) -> Record {
        Record::from_rdata(
            Name::from_utf8(owner).unwrap(),
            60,
            RData::A(rdata::A(std::net::Ipv4Addr::new(192, 0, 2, 1))),
        )
    }

    #[test]
    fn cname_chain_walks_in_order() {
        // Answer section deliberately out of order — the walk must follow the
        // owner→target links, not record order.
        let records = vec![
            cname_rec("cdn.example.net.", "edge.host.example."),
            a_rec("edge.host.example."),
            cname_rec("www.example.com.", "cdn.example.net."),
        ];
        let chain = extract_cname_chain("www.example.com", &records);
        assert_eq!(chain, vec!["cdn.example.net", "edge.host.example"]);
    }

    #[test]
    fn cname_chain_empty_when_no_cnames() {
        let records = vec![a_rec("example.com.")];
        assert!(extract_cname_chain("example.com", &records).is_empty());
    }

    #[test]
    fn cname_chain_is_case_insensitive_and_ignores_root_dot() {
        let records = vec![cname_rec("WWW.Example.COM.", "CDN.Example.NET.")];
        let chain = extract_cname_chain("www.example.com", &records);
        assert_eq!(chain, vec!["cdn.example.net"]);
    }

    #[test]
    fn cname_chain_loop_terminates() {
        let records = vec![
            cname_rec("a.example.", "b.example."),
            cname_rec("b.example.", "a.example."),
        ];
        let chain = extract_cname_chain("a.example", &records);
        // Walk stops when it would revisit a name — no infinite loop.
        assert_eq!(chain, vec!["b.example", "a.example"]);
    }

    #[test]
    fn cname_chain_ignores_unrelated_cnames() {
        let records = vec![cname_rec("other.example.", "target.example.")];
        assert!(extract_cname_chain("www.example.com", &records).is_empty());
    }

    // ── resolve_detailed ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn resolve_detailed_populates_per_type_fields() {
        let (ips, r) = resolve_detailed("localhost", false, false).await.unwrap();
        assert!(!ips.is_empty());
        assert!(r.success);
        // Both lookups performed → both timings and counts present.
        assert!(r.a_ms.is_some(), "a_ms must be recorded");
        assert!(r.aaaa_ms.is_some(), "aaaa_ms must be recorded");
        assert!(r.a_record_count.is_some());
        assert!(r.aaaa_record_count.is_some());
        // localhost resolves directly, no CNAME chain.
        assert!(r.cname_chain.is_empty());
        assert!(r.resolver.is_some());
    }

    #[tokio::test]
    async fn resolve_detailed_ipv4_only_skips_aaaa() {
        let (ips, r) = resolve_detailed("localhost", true, false).await.unwrap();
        assert!(ips.iter().all(|ip| ip.is_ipv4()));
        assert!(r.a_ms.is_some());
        assert!(r.aaaa_ms.is_none(), "AAAA lookup must be skipped");
        assert!(r.aaaa_record_count.is_none());
    }

    #[tokio::test]
    async fn resolve_detailed_unresolvable_returns_error() {
        let result = resolve_detailed("this-hostname-does-not-exist.invalid", false, false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn run_dns_probe_carries_detailed_fields() {
        let attempt = run_dns_probe(uuid::Uuid::new_v4(), 0, "localhost", false, false).await;
        assert!(attempt.success, "probe should succeed: {:?}", attempt.error);
        let dns = attempt.dns.expect("dns result");
        assert!(dns.a_ms.is_some());
        assert!(dns.aaaa_ms.is_some());
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
