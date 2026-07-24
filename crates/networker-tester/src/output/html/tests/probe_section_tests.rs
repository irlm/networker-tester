//! Depth-probe report cards (rpm / ping / path / dualstack / websocket /
//! pmtud) and the DNS depth card. Each section has a populated-render
//! assertion and an absent-data-no-render assertion — the sections are
//! data-gated so runs without those modes stay byte-identical (see
//! tests/html_snapshot.rs for the byte-level guarantee).

use super::*;

// ─────────────────────────────────────────────────────────────────────────
// Fixture builders
// ─────────────────────────────────────────────────────────────────────────

fn make_rpm_attempt(bufferbloat_factor: Option<f64>) -> RequestAttempt {
    let mut a = make_attempt(Protocol::Rpm, true);
    a.rpm = Some(crate::metrics::RpmResult {
        remote_addr: "10.0.0.1:9999".into(),
        unloaded_probe_count: 20,
        unloaded_success_count: 20,
        unloaded_loss_percent: 0.0,
        unloaded_rtt_min_ms: 1.0,
        unloaded_rtt_avg_ms: 2.0,
        unloaded_rtt_p95_ms: 3.0,
        unloaded_jitter_ms: 0.3,
        loaded_probe_count: 20,
        loaded_success_count: 19,
        loaded_loss_percent: 5.0,
        loaded_rtt_min_ms: 4.0,
        loaded_rtt_avg_ms: 8.0,
        loaded_rtt_p95_ms: 16.0,
        loaded_jitter_ms: 1.2,
        rpm: Some(7500.0),
        bufferbloat_factor,
        load_duration_ms: 5000.0,
        load_bytes_transferred: 100_000_000,
        load_downloads_completed: 4,
        load_throughput_mbps: Some(20.0),
        started_at: Utc::now(),
    });
    a
}

fn make_ping_attempt(success_count: u32, reply_ttl: Option<u32>) -> RequestAttempt {
    let mut a = make_attempt(Protocol::Ping, true);
    let lost = 10 - success_count;
    a.ping = Some(crate::metrics::PingResult {
        remote_addr: "192.0.2.7".into(),
        probe_count: 10,
        success_count,
        loss_percent: lost as f64 * 10.0,
        rtt_min_ms: if success_count > 0 { 1.1 } else { 0.0 },
        rtt_avg_ms: if success_count > 0 { 2.2 } else { 0.0 },
        rtt_p95_ms: if success_count > 0 { 3.3 } else { 0.0 },
        jitter_ms: if success_count > 0 { 0.4 } else { 0.0 },
        probe_rtts_ms: vec![Some(2.2); success_count as usize],
        reply_ttl,
        started_at: Utc::now(),
    });
    a
}

fn make_path_attempt(with_hops: bool) -> RequestAttempt {
    let mut a = make_attempt(Protocol::Path, true);
    let hops = if with_hops {
        vec![
            crate::metrics::PathHop {
                index: 1,
                addr: Some("192.168.1.1".into()),
                rtt_ms: Some(0.8),
            },
            crate::metrics::PathHop {
                index: 2,
                addr: None,
                rtt_ms: None,
            },
            crate::metrics::PathHop {
                index: 3,
                addr: Some("203.0.113.9".into()),
                rtt_ms: Some(12.5),
            },
        ]
    } else {
        vec![]
    };
    a.path = Some(crate::metrics::PathResult {
        remote_addr: "203.0.113.9:443".into(),
        hops,
        hop_count: Some(3),
        destination_reached: true,
        destination_rtt_ms: Some(12.5),
        method: if with_hops {
            "udp-ttl/ip-recverr".into()
        } else {
            "udp-ttl-estimate".into()
        },
        max_ttl: 30,
        started_at: Utc::now(),
    });
    a
}

fn make_dualstack_attempt() -> RequestAttempt {
    let mut a = make_attempt(Protocol::DualStack, true);
    a.dualstack = Some(crate::metrics::DualStackResult {
        ipv4: crate::metrics::DualStackLeg {
            attempted: true,
            success: true,
            addr: Some("192.0.2.10:443".into()),
            dns_ms: Some(1.0),
            tcp_ms: Some(2.0),
            tls_ms: Some(6.0),
            ttfb_ms: Some(11.0),
            total_ms: Some(15.0),
            error: None,
        },
        ipv6: crate::metrics::DualStackLeg {
            attempted: true,
            success: true,
            addr: Some("[2001:db8::10]:443".into()),
            dns_ms: Some(1.5),
            tcp_ms: Some(2.5),
            tls_ms: Some(6.5),
            ttfb_ms: Some(13.0),
            total_ms: Some(18.0),
            error: None,
        },
        faster_family: Some("ipv4".into()),
        delta_ms: Some(3.0),
        happy_eyeballs_verdict: "ipv6 (connect within 250ms grace of ipv4)".into(),
        happy_eyeballs_grace_ms: 250.0,
        started_at: Utc::now(),
    });
    a
}

fn make_websocket_attempt(echo_count: u32) -> RequestAttempt {
    let mut a = make_attempt(Protocol::WebSocket, true);
    a.websocket = Some(crate::metrics::WebSocketResult {
        url: "wss://localhost:8443/ws/echo".into(),
        upgrade_ms: 4.25,
        upgrade_status: Some(101),
        message_count: 20,
        echo_count,
        loss_percent: 100.0 * (20 - echo_count) as f64 / 20.0,
        msg_rtt_min_ms: if echo_count > 0 { 0.9 } else { 0.0 },
        msg_rtt_avg_ms: if echo_count > 0 { 1.8 } else { 0.0 },
        msg_rtt_p95_ms: if echo_count > 0 { 3.6 } else { 0.0 },
        jitter_ms: if echo_count > 0 { 0.25 } else { 0.0 },
        msg_rtts_ms: vec![Some(1.8); echo_count as usize],
        payload_size: 64,
        started_at: Utc::now(),
    });
    a
}

fn make_pmtud_attempt(path_mtu: Option<u32>, lower_bound_only: bool) -> RequestAttempt {
    let mut a = make_attempt(Protocol::Pmtud, true);
    a.pmtud = Some(crate::metrics::PmtudResult {
        remote_addr: "203.0.113.9:9999".into(),
        path_mtu,
        max_unfragmented_payload: path_mtu.map(|m| m - 28),
        probes_sent: 11,
        method: "df-udp-echo/ip-recverr".into(),
        icmp_mtu: Some(1492),
        local_mtu: Some(1500),
        header_bytes: 28,
        lower_bound_only,
        started_at: Utc::now(),
    });
    a
}

fn make_dns_depth_attempt() -> RequestAttempt {
    let mut a = make_attempt(Protocol::Dns, true);
    a.dns = Some(crate::metrics::DnsResult {
        query_name: "www.example.com".into(),
        resolved_ips: vec!["192.0.2.1".into()],
        duration_ms: 12.0,
        started_at: Utc::now(),
        success: true,
        resolver: Some("system (192.168.1.1:53)".into()),
        a_ms: Some(7.5),
        aaaa_ms: Some(9.25),
        a_record_count: Some(2),
        aaaa_record_count: Some(1),
        cname_chain: vec!["cdn.example.net".into(), "edge.example.org".into()],
    });
    a
}

/// A `dns` attempt WITHOUT the wave-1 depth fields (pre-0.28.76 shape /
/// non-dns-mode resolve) — must not trigger the DNS Depth card.
fn make_plain_dns_attempt() -> RequestAttempt {
    let mut a = make_attempt(Protocol::Dns, true);
    a.dns = Some(crate::metrics::DnsResult {
        query_name: "www.example.com".into(),
        resolved_ips: vec!["192.0.2.1".into()],
        duration_ms: 12.0,
        started_at: Utc::now(),
        success: true,
        resolver: None,
        a_ms: None,
        aaaa_ms: None,
        a_record_count: None,
        aaaa_record_count: None,
        cname_chain: Vec::new(),
    });
    a
}

// ─────────────────────────────────────────────────────────────────────────
// rpm
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn rpm_section_renders_with_data() {
    let mut run = make_run();
    run.attempts.clear();
    run.attempts.push(make_rpm_attempt(Some(4.0)));
    let html = render(&run, None, None);
    assert!(
        html.contains("Latency Under Load (RPM)"),
        "rpm card must appear"
    );
    assert!(html.contains("Unloaded"), "unloaded phase row must appear");
    assert!(html.contains("Loaded"), "loaded phase row must appear");
    assert!(
        html.contains("<strong>7500</strong> round-trips/min"),
        "RPM headline must appear"
    );
    assert!(html.contains("20.00 MB/s"), "load throughput must appear");
    // Loaded avg RTT (8.00ms) and unloaded avg (2.00ms) both present.
    assert!(html.contains("8.00ms") && html.contains("2.00ms"));
}

#[test]
fn rpm_bufferbloat_factor_above_2_uses_warn_class() {
    let mut run = make_run();
    run.attempts.clear();
    run.attempts.push(make_rpm_attempt(Some(4.0)));
    let html = render(&run, None, None);
    assert!(
        html.contains(r#"<span class="warn">4.00x"#),
        "factor > 2 must use warn styling"
    );
}

#[test]
fn rpm_bufferbloat_factor_below_2_uses_ok_class() {
    let mut run = make_run();
    run.attempts.clear();
    run.attempts.push(make_rpm_attempt(Some(1.2)));
    let html = render(&run, None, None);
    assert!(
        html.contains(r#"<span class="ok">1.20x</span>"#),
        "factor <= 2 must use ok styling"
    );
}

#[test]
fn rpm_section_absent_without_data() {
    let run = make_run(); // http1-only fixture
    let html = render(&run, None, None);
    assert!(
        !html.contains("Latency Under Load"),
        "rpm card must not render without rpm data"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// ping
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn ping_section_renders_with_data() {
    let mut run = make_run();
    run.attempts.clear();
    run.attempts.push(make_ping_attempt(9, Some(64)));
    let html = render(&run, None, None);
    assert!(
        html.contains("ICMP Ping Statistics"),
        "ping card must appear"
    );
    assert!(html.contains("192.0.2.7"), "target address must appear");
    assert!(html.contains("2.20ms"), "avg RTT must appear");
    assert!(html.contains("Reply TTL"), "reply TTL column must appear");
    assert!(html.contains("<td>64</td>"), "TTL value must appear");
    assert!(
        html.contains(r#"class="warn">10.0%"#),
        "nonzero loss must use warn class"
    );
}

#[test]
fn ping_all_lost_shows_dashes_not_zero_sentinels() {
    let mut run = make_run();
    run.attempts.clear();
    run.attempts.push(make_ping_attempt(0, None));
    let html = render(&run, None, None);
    assert!(html.contains("ICMP Ping Statistics"));
    assert!(
        !html.contains("0.00ms"),
        "fully-lost attempt must not show 0.0 sentinel RTTs"
    );
}

#[test]
fn ping_section_absent_without_data() {
    let run = make_run();
    let html = render(&run, None, None);
    assert!(
        !html.contains("ICMP Ping Statistics"),
        "ping card must not render without ping data"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// path
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn path_section_renders_hop_table_when_hops_exist() {
    let mut run = make_run();
    run.attempts.clear();
    run.attempts.push(make_path_attempt(true));
    let html = render(&run, None, None);
    assert!(html.contains("Network Path"), "path card must appear");
    assert!(html.contains("192.168.1.1"), "hop address must appear");
    assert!(html.contains("12.50"), "hop RTT must appear");
    assert!(html.contains("udp-ttl/ip-recverr"), "method must appear");
    // Silent hop (index 2) renders an em dash, not an invented address.
    assert!(html.contains("<td>—</td>"), "silent hop must show em dash");
    assert!(
        !html.contains("not observable"),
        "degraded-mode note must not appear when hops exist"
    );
}

#[test]
fn path_section_degraded_mode_renders_verdict_without_hop_table() {
    let mut run = make_run();
    run.attempts.clear();
    run.attempts.push(make_path_attempt(false));
    let html = render(&run, None, None);
    assert!(html.contains("Network Path"), "path card must appear");
    assert!(
        html.contains("udp-ttl-estimate"),
        "degraded method must appear"
    );
    assert!(
        html.contains("not observable unprivileged"),
        "degraded-mode note must appear"
    );
    assert!(
        !html.contains("<th>Hop</th>"),
        "hop table must NOT render when no hops were observed — hops are never invented"
    );
    assert!(
        html.contains(r#"<span class="ok">reached</span>"#),
        "destination-reached verdict must appear"
    );
}

#[test]
fn path_section_absent_without_data() {
    let run = make_run();
    let html = render(&run, None, None);
    assert!(
        !html.contains("Network Path"),
        "path card must not render without path data"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// dualstack
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn dualstack_section_renders_comparison_table() {
    let mut run = make_run();
    run.attempts.clear();
    run.attempts.push(make_dualstack_attempt());
    let html = render(&run, None, None);
    assert!(
        html.contains("Dual-Stack (IPv4 vs IPv6)"),
        "dualstack card must appear"
    );
    assert!(html.contains("<th>IPv4</th>") && html.contains("<th>IPv6</th>"));
    // Per-phase rows with averaged timings from both legs.
    assert!(html.contains("15.00ms"), "IPv4 total must appear");
    assert!(html.contains("18.00ms"), "IPv6 total must appear");
    assert!(
        html.contains("<strong>ipv4</strong> by 3.00ms avg"),
        "faster family + delta must appear"
    );
    assert!(
        html.contains("ipv6 (connect within 250ms grace of ipv4)"),
        "happy-eyeballs verdict must appear"
    );
    assert!(
        html.contains("[2001:db8::10]:443"),
        "IPv6 leg address must appear"
    );
}

#[test]
fn dualstack_section_absent_without_data() {
    let run = make_run();
    let html = render(&run, None, None);
    assert!(
        !html.contains("Dual-Stack"),
        "dualstack card must not render without dualstack data"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// websocket
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn websocket_section_renders_upgrade_and_rtt_stats() {
    let mut run = make_run();
    run.attempts.clear();
    run.attempts.push(make_websocket_attempt(19));
    let html = render(&run, None, None);
    assert!(html.contains("<h2>WebSocket</h2>"), "ws card must appear");
    assert!(html.contains("4.25"), "upgrade_ms must appear");
    assert!(
        html.contains(r#"<span class="ok">101</span>"#),
        "101 upgrade status must use ok class"
    );
    assert!(html.contains("1.80ms"), "msg RTT avg must appear");
    assert!(
        html.contains(r#"class="warn">5.0%"#),
        "nonzero loss must use warn class"
    );
    assert!(
        html.contains("wss://localhost:8443/ws/echo"),
        "url must appear in the note"
    );
}

#[test]
fn websocket_all_lost_shows_dashes_not_zero_sentinels() {
    let mut run = make_run();
    run.attempts.clear();
    run.attempts.push(make_websocket_attempt(0));
    let html = render(&run, None, None);
    assert!(html.contains("<h2>WebSocket</h2>"));
    assert!(
        !html.contains("0.00ms"),
        "fully-lost attempt must not show 0.0 sentinel RTTs"
    );
}

#[test]
fn websocket_section_absent_without_data() {
    let run = make_run();
    let html = render(&run, None, None);
    assert!(
        !html.contains("<h2>WebSocket</h2>"),
        "ws card must not render without websocket data"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// pmtud
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn pmtud_section_renders_verdict_and_contrast() {
    let mut run = make_run();
    run.attempts.clear();
    run.attempts.push(make_pmtud_attempt(Some(1492), false));
    let html = render(&run, None, None);
    assert!(
        html.contains("Path MTU Discovery"),
        "pmtud card must appear"
    );
    assert!(
        html.contains("<strong>1492 bytes</strong>"),
        "path MTU must appear"
    );
    assert!(
        html.contains("max unfragmented payload 1464 + 28 header"),
        "payload/header breakdown must appear"
    );
    assert!(
        html.contains("df-udp-echo/ip-recverr"),
        "method must appear"
    );
    assert!(html.contains("1500 bytes"), "local MTU must appear");
    assert!(
        html.contains("path is narrower than the local link"),
        "narrower-path contrast note must appear"
    );
}

#[test]
fn pmtud_lower_bound_only_flagged() {
    let mut run = make_run();
    run.attempts.clear();
    run.attempts.push(make_pmtud_attempt(Some(1500), true));
    let html = render(&run, None, None);
    assert!(
        html.contains("&ge;1500 bytes"),
        "lower-bound MTU must render with >= prefix"
    );
    assert!(
        html.contains("lower bound only"),
        "lower_bound_only flag must be surfaced"
    );
}

#[test]
fn pmtud_unknown_mtu_is_honest() {
    let mut run = make_run();
    run.attempts.clear();
    run.attempts.push(make_pmtud_attempt(None, false));
    let html = render(&run, None, None);
    assert!(
        html.contains(r#"<span class="warn">unknown</span>"#),
        "no-feedback verdict must say unknown, not a number"
    );
}

#[test]
fn pmtud_section_absent_without_data() {
    let run = make_run();
    let html = render(&run, None, None);
    assert!(
        !html.contains("Path MTU Discovery"),
        "pmtud card must not render without pmtud data"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// DNS depth
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn dns_depth_section_renders_with_detail() {
    let mut run = make_run();
    run.attempts.clear();
    run.attempts.push(make_dns_depth_attempt());
    let html = render(&run, None, None);
    assert!(html.contains("DNS Depth"), "dns depth card must appear");
    assert!(
        html.contains("7.50ms avg") && html.contains("2 records"),
        "A timing + record count must appear"
    );
    assert!(
        html.contains("9.25ms avg") && html.contains("1 record"),
        "AAAA timing + record count must appear"
    );
    assert!(
        html.contains("cdn.example.net") && html.contains("edge.example.org"),
        "CNAME chain must appear"
    );
    assert!(
        html.contains("www.example.com"),
        "query name must lead the chain"
    );
}

#[test]
fn dns_depth_section_absent_for_plain_dns_result() {
    let mut run = make_run();
    run.attempts.clear();
    run.attempts.push(make_plain_dns_attempt());
    let html = render(&run, None, None);
    assert!(
        !html.contains("DNS Depth"),
        "dns depth card must not render when the depth fields are unset"
    );
}

#[test]
fn dns_depth_section_absent_without_dns_data() {
    let run = make_run();
    let html = render(&run, None, None);
    assert!(
        !html.contains("DNS Depth"),
        "dns depth card must not render without dns attempts"
    );
}
