use std::collections::BTreeSet;
use std::path::Path;

use crate::metrics::{
    attempt_payload_bytes, compute_stats, primary_metric_label, primary_metric_value,
    BrowserResult, PageLoadResult, Protocol, RequestAttempt, TestRun, UrlTestRun,
};

pub fn fmt_bytes(n: usize) -> String {
    if n >= 1 << 30 {
        format!("{:.1}GiB", n as f64 / (1u64 << 30) as f64)
    } else if n >= 1 << 20 {
        format!("{:.0}MiB", n as f64 / (1u64 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.0}KiB", n as f64 / (1u64 << 10) as f64)
    } else {
        format!("{n}B")
    }
}

pub fn print_summary(run: &TestRun) {
    let ok = run.success_count();
    let fail = run.failure_count();
    let total = run.attempts.len();

    // Stats tables below compute over measured-phase attempts only, so the
    // console agrees with the benchmark JSON artifact (warmup/overhead/
    // pilot/cooldown samples are excluded). Non-benchmark runs are
    // unaffected: `measured` is then all attempts.
    let measured = run.measured_attempts();
    let excluded_from_stats = run.attempts.len() - measured.len();

    // Extract server version from the first attempt that reported it.
    let server_version: String = run
        .attempts
        .iter()
        .find_map(|a| {
            a.server_timing
                .as_ref()
                .and_then(|st| st.server_version.as_deref())
        })
        .unwrap_or("—")
        .to_string();

    println!("\n══════════════════════════════════════════════");
    println!(" Networker Tester – Run {}", run.run_id);
    println!("══════════════════════════════════════════════");
    println!(" Target         : {}", run.target_url);
    println!(" Modes          : {}", run.modes.join(", "));
    println!(" Results        : {ok}/{total} succeeded  ({fail} failed)");
    println!(" Client version : {}", run.client_version);
    println!(" Server version : {server_version}");

    // Offline GeoIP enrichment (one line max; only when a MaxMind DB matched).
    if run.client_geo.is_some() || run.target_geo.is_some() {
        let side = |geo: &Option<crate::metrics::GeoInfo>| {
            geo.as_ref().map_or_else(|| "—".to_string(), |g| g.label())
        };
        let db_date = run
            .client_geo
            .as_ref()
            .or(run.target_geo.as_ref())
            .and_then(|g| g.db_date.as_deref())
            .unwrap_or("?");
        println!(
            " Geo            : client {} → target {} (GeoIP db {db_date})",
            side(&run.client_geo),
            side(&run.target_geo),
        );
    }

    if let Some(fin) = run.finished_at {
        let dur = (fin - run.started_at).num_milliseconds();
        println!(" Duration       : {dur}ms total");
    }

    // Noisy-tester flag (measurement gap #15): warn when the 1-minute load
    // average exceeded the core count at either run-level load sample.
    if let Some(cores) = run.client_info.as_ref().map(|i| i.cpu_cores as f64) {
        let overloaded = [&run.client_load_before, &run.client_load_after]
            .iter()
            .filter_map(|s| s.as_ref().and_then(|s| s.load_avg_1m))
            .any(|load| load > cores);
        if overloaded {
            println!(" ⚠ tester under load — measurements may be noisy");
        }
    }

    // CPU-contention flag: the sampled peaks (max/p95 busy) catch bursts the
    // whole-run mean hides; any hypervisor steal means the vCPU was preempted
    // mid-measurement (cloud testers).
    if let Some(cpu) = run.cpu_usage.as_ref() {
        let busy_hot = [cpu.max_busy_percent, cpu.p95_busy_percent]
            .iter()
            .flatten()
            .any(|v| *v > 80.0);
        let steal_hot = [cpu.mean_steal_percent, cpu.max_steal_percent]
            .iter()
            .flatten()
            .any(|v| *v > 1.5);
        if busy_hot || steal_hot {
            println!(" ⚠ tester CPU-contended — measurements may be noisy");
        }
    }

    // Build (proto, Option<payload_bytes>) groups in canonical protocol order.
    let ordered_protos = [
        Protocol::Http1,
        Protocol::Http2,
        Protocol::Http3,
        Protocol::Native,
        Protocol::Curl,
        Protocol::SdkProbe,
        Protocol::Tcp,
        Protocol::Udp,
        Protocol::Rpm,
        Protocol::Responsiveness,
        Protocol::Stamp,
        Protocol::Ping,
        Protocol::Path,
        Protocol::DualStack,
        Protocol::WebSocket,
        Protocol::Pmtud,
        Protocol::Dns,
        Protocol::Tls,
        Protocol::TlsResume,
        Protocol::Download,
        Protocol::Download1,
        Protocol::Download2,
        Protocol::Download3,
        Protocol::Upload,
        Protocol::Upload1,
        Protocol::Upload2,
        Protocol::Upload3,
        Protocol::WebDownload,
        Protocol::WebUpload,
        Protocol::UdpDownload,
        Protocol::UdpUpload,
        Protocol::Mthroughput,
        Protocol::PageLoad,
        Protocol::PageLoad2,
        Protocol::PageLoad3,
        Protocol::Browser,
        Protocol::Browser1,
        Protocol::Browser2,
        Protocol::Browser3,
    ];
    let stat_groups: Vec<(Protocol, Option<usize>)> = ordered_protos
        .iter()
        .flat_map(|proto| {
            let payloads: BTreeSet<Option<usize>> = measured
                .iter()
                .filter(|a| &a.protocol == proto)
                .map(|a| attempt_payload_bytes(a))
                .collect();
            payloads.into_iter().map(move |p| (proto.clone(), p))
        })
        .collect();

    let group_label = |proto: &Protocol, payload: Option<usize>| match payload {
        None => proto.to_string(),
        Some(b) => format!("{proto} {}", fmt_bytes(b)),
    };

    // Per-protocol/payload averages table
    println!(
        "\n {:<16} │ #   │ Avg DNS │ Avg TCP │ Avg TLS │ Avg TTFB │ Avg Total",
        "Protocol"
    );
    println!("──────────────────┼─────┼─────────┼─────────┼─────────┼──────────┼───────────");

    for (proto, payload) in &stat_groups {
        let rows: Vec<_> = measured
            .iter()
            .filter(|a| &a.protocol == proto && attempt_payload_bytes(a) == *payload)
            .collect();
        if rows.is_empty() {
            continue;
        }

        let avg_f = |f: fn(&RequestAttempt) -> Option<f64>| -> String {
            let vals: Vec<f64> = rows.iter().filter_map(|a| f(a)).collect();
            if vals.is_empty() {
                "—".into()
            } else {
                format!("{:.1}ms", vals.iter().sum::<f64>() / vals.len() as f64)
            }
        };

        println!(
            " {label:<16} │ {n:<3} │ {dns:<7} │ {tcp:<7} │ {tls:<7} │ {ttfb:<8} │ {total}",
            label = group_label(proto, *payload),
            n = rows.len(),
            dns = avg_f(|a| a.dns.as_ref().map(|d| d.duration_ms)),
            tcp = avg_f(|a| a.tcp.as_ref().map(|t| t.connect_duration_ms)),
            tls = avg_f(|a| a.tls.as_ref().map(|t| t.handshake_duration_ms)),
            ttfb = avg_f(|a| a.http.as_ref().map(|h| h.ttfb_ms)),
            total = avg_f(|a| {
                a.http
                    .as_ref()
                    .map(|h| h.total_duration_ms)
                    .or_else(|| a.udp.as_ref().map(|u| u.rtt_avg_ms))
                    .or_else(|| a.udp_throughput.as_ref().map(|ut| ut.transfer_ms))
            }),
        );
    }

    // Per-group statistics (primary metric: ms for latency, MB/s for throughput)
    let has_stats = stat_groups.iter().any(|(proto, payload)| {
        measured
            .iter()
            .filter(|a| &a.protocol == proto && attempt_payload_bytes(a) == *payload)
            .any(|a| primary_metric_value(a).is_some())
    });
    if has_stats {
        println!();
        println!(
            " {:<16} │ Metric           │  N  │    Min   │   Mean   │   p50    │   p95    │   p99    │    Max   │  StdDev",
            "Protocol"
        );
        println!(
            "──────────────────┼──────────────────┼─────┼──────────┼──────────┼──────────┼──────────┼──────────┼──────────┼─────────"
        );
        for (proto, payload) in &stat_groups {
            let vals: Vec<f64> = measured
                .iter()
                .filter(|a| &a.protocol == proto && attempt_payload_bytes(a) == *payload)
                .filter_map(|a| primary_metric_value(a))
                .collect();
            if let Some(s) = compute_stats(&vals) {
                let label = primary_metric_label(proto);
                // p95/p99 are suppressed below the sample-size guard
                // (n≥20 / n≥100) — printing them at small n would present the
                // max as a tail estimate.
                let fmt_pctl = |v: Option<f64>| {
                    v.map_or_else(|| format!("{:>8}", "—"), |x| format!("{x:>8.2}"))
                };
                println!(
                    " {grp:<16} │ {label:<16} │ {n:<3} │ {min:>8.2} │ {mean:>8.2} │ {p50:>8.2} │ {p95} │ {p99} │ {max:>8.2} │ {stddev:>7.2}",
                    grp = group_label(proto, *payload),
                    n = s.count,
                    min = s.min,
                    mean = s.mean,
                    p50 = s.p50,
                    p95 = fmt_pctl(s.p95),
                    p99 = fmt_pctl(s.p99),
                    max = s.max,
                    stddev = s.stddev,
                );
            }
        }
    }

    // Human surfaces must agree with the benchmark artifact: state what the
    // phase filter dropped from the tables above.
    if excluded_from_stats > 0 {
        println!(
            "\n Note: {excluded_from_stats} warmup/overhead/pilot/cooldown attempt{s} excluded from stats (benchmark phase filter)",
            s = if excluded_from_stats == 1 { "" } else { "s" },
        );
    }

    // Post-transfer TCP kernel stats note (gap #5): sampled after each
    // HTTP-family transfer completes. Only printed when segments actually
    // retransmitted — silence means clean transfers.
    print_retransmission_note(run);

    // QUIC analogue (deep-measurement M1 B.1): post-transfer quinn stats.
    // Only printed when packets were actually declared lost.
    print_quic_loss_note(run);

    // UDP loss-pattern note (RFC 3357, deep-measurement M4 §2.4): only when a
    // probe train showed a burst — bursty loss points at congestion/buffer
    // events, random-scattered loss at path noise. Silence = clean or random.
    print_loss_pattern_note(run);
    // Throughput-attribution triad (B.2): when the kernel chronographs show
    // the transfer was receiver-window- or send-buffer-limited, say so — the
    // throughput number alone blames the path for a bottleneck that wasn't.
    print_throughput_attribution_note(run);

    // UDP local-drop split (B.6): datagrams dropped by OUR socket's receive
    // buffer are counted inside loss% — flag them so local overflow is not
    // read as path loss.
    print_udp_local_drop_note(run);

    // sdkprobe network-vs-server latency split — the core "find the main
    // issue" breakdown. Only rendered when a sdkprobe run produced a split.
    print_sdk_split(run);

    // rpm latency-under-load breakdown — unloaded vs loaded RTT, bufferbloat
    // factor, and RPM. Only rendered when an rpm attempt produced a result.
    print_rpm_summary(run);

    // Draft-conformant responsiveness breakdown (per-direction RPM, capacity
    // at saturation, probe trimmed means) and STAMP directional detail —
    // rendered only when those probes produced results.
    print_responsiveness_summary(run);
    print_stamp_summary(run);

    // Multi-connection capacity breakdown (aggregate capacity, per-connection
    // fair-share spread, TCP-attribution verdicts) — rendered only when the
    // probe produced a result.
    print_mthroughput_summary(run);

    // path hop table and dualstack family comparison — rendered only when
    // those probes produced results.
    print_path_summary(run);
    print_dualstack_summary(run);

    // Per-record-type DNS detail (dns mode) and certificate/OCSP detail
    // (tls mode) — rendered only when the probes captured the extra depth.
    print_dns_detail(run);
    print_tls_detail(run);
    // HTTP/3 QUIC session-resumption / 0-RTT note — only rendered when an
    // http3 attempt actually carried the measurement.
    print_h3_zero_rtt(run);

    // websocket upgrade + message-RTT breakdown and pmtud path-MTU verdict —
    // rendered only when those probes produced results.
    print_websocket_summary(run);
    print_pmtud_summary(run);

    // Protocol comparison table when any pageload or browser variant is present
    let has_pageload = run.attempts.iter().any(|a| {
        matches!(
            a.protocol,
            Protocol::PageLoad
                | Protocol::PageLoad2
                | Protocol::PageLoad3
                | Protocol::Browser
                | Protocol::Browser1
                | Protocol::Browser2
                | Protocol::Browser3
        )
    });
    if has_pageload {
        print_comparison(run);
    }

    println!("══════════════════════════════════════════════\n");
}

/// Warn when post-transfer TCP kernel stats (`http.socket_stats`, sampled on a
/// dup of the probe socket after the transfer) show retransmitted segments —
/// the single most common explanation for a throughput anomaly. Quiet when no
/// attempt retransmitted or the platform reports no kernel stats (Windows).
fn print_retransmission_note(run: &TestRun) {
    let mut attempts_with_retrans = 0usize;
    let mut total_retrans: u64 = 0;
    let mut pageload_conns_with_retrans = 0usize;
    let mut algos: BTreeSet<String> = BTreeSet::new();
    let retrans_of = |s: &crate::metrics::SocketStats| -> u64 {
        s.total_retrans.unwrap_or(0).max(s.retransmits.unwrap_or(0)) as u64
    };
    for a in &run.attempts {
        if let Some(s) = a.http.as_ref().and_then(|h| h.socket_stats.as_ref()) {
            if let Some(algo) = &s.congestion_algorithm {
                algos.insert(algo.clone());
            }
            let n = retrans_of(s);
            if n > 0 {
                attempts_with_retrans += 1;
                total_retrans += n;
            }
        }
        // Pageload probes additionally sample every pooled connection; the
        // manifest connection's stats are already counted via http above.
        if let Some(pl) = a.page_load.as_ref() {
            for (idx, s) in pl.per_connection_socket_stats.iter().enumerate() {
                if let Some(algo) = &s.congestion_algorithm {
                    algos.insert(algo.clone());
                }
                let n = retrans_of(s);
                if n > 0 {
                    pageload_conns_with_retrans += 1;
                    // Connection 0 duplicates http.socket_stats — don't
                    // double-count its segments in the total.
                    let already_counted = idx == 0
                        && a.http
                            .as_ref()
                            .and_then(|h| h.socket_stats.as_ref())
                            .is_some_and(|hs| retrans_of(hs) > 0);
                    if !already_counted {
                        total_retrans += n;
                    }
                }
            }
        }
    }
    if attempts_with_retrans > 0 || pageload_conns_with_retrans > 0 {
        let algo_note = if algos.is_empty() {
            String::new()
        } else {
            format!(
                " (congestion control: {})",
                algos.into_iter().collect::<Vec<_>>().join(", ")
            )
        };
        let pageload_note = if pageload_conns_with_retrans > 0 {
            format!("; incl. {pageload_conns_with_retrans} pageload connection(s)")
        } else {
            String::new()
        };
        println!(
            "\n ⚠ TCP retransmissions during transfer: {total_retrans} segment(s) across \
             {attempts_with_retrans} attempt(s){pageload_note}{algo_note} — throughput numbers may reflect loss recovery"
        );
    }
}

/// QUIC analogue of [`print_retransmission_note`]: warn when the post-transfer
/// `quinn::Connection::stats()` snapshot (`http.quic_stats`, deep-measurement
/// M1 B.1) shows packets declared lost — the h3-side explanation for a
/// throughput anomaly. Counts the primary connection only (the resumption
/// follow-up's tiny exchange is not part of the measured transfer). Quiet when
/// no h3 attempt lost packets.
fn print_quic_loss_note(run: &TestRun) {
    let mut attempts_with_loss = 0usize;
    let mut total_lost: u64 = 0;
    let mut total_lost_bytes: u64 = 0;
    for a in &run.attempts {
        if let Some(q) = a.http.as_ref().and_then(|h| h.quic_stats.as_ref()) {
            let n = q.lost_packets.unwrap_or(0);
            if n > 0 {
                attempts_with_loss += 1;
                total_lost += n;
                total_lost_bytes += q.lost_bytes.unwrap_or(0);
            }
        }
    }
    if attempts_with_loss > 0 {
        println!(
            "\n ⚠ QUIC packet loss during transfer: {total_lost} packet(s) / {total_lost_bytes} byte(s) \
             across {attempts_with_loss} attempt(s) — h3 throughput numbers may reflect loss recovery"
        );
    }
}

/// UDP loss-pattern note (RFC 3357): warn when any probe train's loss was
/// classified `bursty` or `single-burst` — consecutive losses point at a
/// congestion/buffer event rather than random path noise. Random-like and
/// no-loss trains stay silent (matching the retransmission-note pattern).
fn print_loss_pattern_note(run: &TestRun) {
    let mut bursty = 0usize;
    let mut single_burst = 0usize;
    let mut worst_run = 0u32;
    for a in &run.attempts {
        if let Some(lp) = a.udp.as_ref().and_then(|u| u.loss_pattern.as_ref()) {
            match lp.classification.as_str() {
                "bursty" => bursty += 1,
                "single-burst" => single_burst += 1,
                _ => continue,
            }
            worst_run = worst_run.max(lp.loss_max_burst);
        }
    }
    if bursty + single_burst > 0 {
        println!(
            "\n ⚠ UDP loss was patterned: {bursty} bursty + {single_burst} single-burst \
             train(s), worst run {worst_run} consecutive — congestion/buffer event, not random path noise"
        );
    }
}

/// Aggregate the tcpi_busy_time/rwnd_limited/sndbuf_limited chronographs
/// (Linux ≥ 4.10, sampled post-transfer via `http.socket_stats`) across a
/// run's attempts and produce the attribution line, or `None` when nothing
/// noteworthy: no triad data (non-Linux / old kernels), no busy time, or
/// both limited fractions under 5% (path/CPU-limited transfers are the
/// normal case and stay quiet, matching the retransmission-note pattern).
fn throughput_attribution_line(run: &TestRun) -> Option<String> {
    let triads: Vec<(u64, u64, u64)> = run
        .attempts
        .iter()
        .filter_map(|a| a.http.as_ref().and_then(|h| h.socket_stats.as_ref()))
        .filter_map(|s| match s.busy_time_us {
            Some(b) if b > 0 => Some((
                b,
                s.rwnd_limited_us.unwrap_or(0),
                s.sndbuf_limited_us.unwrap_or(0),
            )),
            _ => None,
        })
        .collect();
    attribution_note(&triads)
}

/// Pure attribution logic over per-attempt `(busy, rwnd_limited,
/// sndbuf_limited)` µs triads (unit-tested directly).
fn attribution_note(triads: &[(u64, u64, u64)]) -> Option<String> {
    let attempts = triads.len();
    let busy_us: u64 = triads.iter().map(|t| t.0).sum();
    let rwnd_us: u64 = triads.iter().map(|t| t.1).sum();
    let sndbuf_us: u64 = triads.iter().map(|t| t.2).sum();
    if busy_us == 0 {
        return None;
    }
    let rwnd_pct = rwnd_us as f64 / busy_us as f64 * 100.0;
    let sndbuf_pct = sndbuf_us as f64 / busy_us as f64 * 100.0;
    const NOTEWORTHY_PCT: f64 = 5.0;
    if rwnd_pct < NOTEWORTHY_PCT && sndbuf_pct < NOTEWORTHY_PCT {
        return None;
    }
    let mut limits: Vec<String> = Vec::new();
    if rwnd_pct >= NOTEWORTHY_PCT {
        limits.push(format!(
            "receiver-window-limited {rwnd_pct:.0}% of the transfer"
        ));
    }
    if sndbuf_pct >= NOTEWORTHY_PCT {
        limits.push(format!(
            "local send-buffer-limited {sndbuf_pct:.0}% of the transfer"
        ));
    }
    let s = if attempts == 1 { "" } else { "s" };
    Some(format!(
        " ⚠ Throughput attribution (kernel tcp_info): {} across {attempts} attempt{s} — \
         the bottleneck was not the network path for that share of the busy time",
        limits.join(", ")
    ))
}

/// Print the throughput-attribution note (quiet when unremarkable).
fn print_throughput_attribution_note(run: &TestRun) {
    if let Some(line) = throughput_attribution_line(run) {
        println!("\n{line}");
    }
}

/// Aggregate UDP local socket drops (B.6) across udp / udpdownload /
/// udpupload / rpm-loaded results; `None` when no attempt observed any.
fn udp_local_drop_line(run: &TestRun) -> Option<String> {
    let per_attempt: Vec<u64> = run
        .attempts
        .iter()
        .map(|a| {
            a.udp
                .as_ref()
                .and_then(|u| u.local_drops)
                .unwrap_or(0)
                .saturating_add(
                    a.udp_throughput
                        .as_ref()
                        .and_then(|u| u.local_drops)
                        .unwrap_or(0),
                )
                .saturating_add(
                    a.rpm
                        .as_ref()
                        .and_then(|r| r.loaded_local_drops)
                        .unwrap_or(0),
                )
        })
        .collect();
    local_drop_note(&per_attempt)
}

/// Pure local-drop warning logic over per-attempt drop counts
/// (unit-tested directly). `None`-unobservable attempts contribute 0.
fn local_drop_note(per_attempt_drops: &[u64]) -> Option<String> {
    let drops: u64 = per_attempt_drops.iter().sum();
    let attempts = per_attempt_drops.iter().filter(|&&d| d > 0).count();
    if drops == 0 {
        return None;
    }
    let s = if drops == 1 { "" } else { "s" };
    let atts = if attempts == 1 { "" } else { "s" };
    Some(format!(
        " ⚠ {drops} datagram{s} dropped locally (socket receive buffer full) across \
         {attempts} attempt{atts} — loss% includes them; this is not path loss"
    ))
}

/// Print the UDP local-drop warning (quiet when no local drops observed).
fn print_udp_local_drop_note(run: &TestRun) {
    if let Some(line) = udp_local_drop_line(run) {
        println!("\n{line}");
    }
}

/// Render the sdkprobe NETWORK-vs-SERVER latency split: the LagHound report's
/// headline breakdown. Averages the per-phase legs (DNS/TCP/TLS from the
/// client, network transfer + server processing from the `Server-Timing: app`
/// split) across all successful sdkprobe attempts and prints where the time
/// went — so an operator can tell at a glance whether the customer's latency
/// is network or the customer's own app.
fn print_sdk_split(run: &TestRun) {
    let sdk: Vec<&RequestAttempt> = run
        .attempts
        .iter()
        .filter(|a| a.protocol == Protocol::SdkProbe && a.success)
        .collect();
    if sdk.is_empty() {
        return;
    }

    // Only meaningful once at least one attempt reported the server split.
    let with_split = sdk
        .iter()
        .filter(|a| {
            a.server_timing
                .as_ref()
                .is_some_and(|st| st.server_ms.is_some())
        })
        .count();
    if with_split == 0 {
        return;
    }

    let avg = |f: &dyn Fn(&RequestAttempt) -> Option<f64>| -> Option<f64> {
        let vals: Vec<f64> = sdk.iter().filter_map(|a| f(a)).collect();
        if vals.is_empty() {
            None
        } else {
            Some(vals.iter().sum::<f64>() / vals.len() as f64)
        }
    };

    let dns = avg(&|a| a.dns.as_ref().map(|d| d.duration_ms));
    let tcp = avg(&|a| a.tcp.as_ref().map(|t| t.connect_duration_ms));
    let tls = avg(&|a| a.tls.as_ref().map(|t| t.handshake_duration_ms));
    let network = avg(&|a| a.server_timing.as_ref().and_then(|st| st.network_ms));
    let server = avg(&|a| a.server_timing.as_ref().and_then(|st| st.server_ms));
    let total = avg(&|a| a.http.as_ref().map(|h| h.total_duration_ms));
    let anomalies = sdk
        .iter()
        .filter(|a| a.server_timing.as_ref().is_some_and(|st| st.split_anomaly))
        .count();

    let line = |label: &str, v: Option<f64>| {
        let val = v.map_or_else(|| "—".to_string(), |x| format!("{x:>8.2}ms"));
        println!("   {label:<18} {val:>12}");
    };

    println!();
    println!(
        " SDK latency split (avg over {n} probe{s}, {ws} with server timing)",
        n = sdk.len(),
        s = if sdk.len() == 1 { "" } else { "s" },
        ws = with_split,
    );
    println!("──────────────────────────────────────────");
    line("DNS", dns);
    line("TCP connect", tcp);
    line("TLS handshake", tls);
    line("Network transfer", network);
    line("Server processing", server);
    line("Total", total);
    if let (Some(net), Some(srv)) = (network, server) {
        let leg = if srv >= net { "SERVER" } else { "NETWORK" };
        println!("   → dominant leg: {leg} (network {net:.1}ms vs server {srv:.1}ms)");
    }
    if anomalies > 0 {
        println!("   ⚠ {anomalies} probe(s) had server_ms > wall — network leg clamped to 0");
    }
}

/// Render the rpm latency-under-load breakdown: unloaded vs loaded UDP echo
/// RTT side by side, the bufferbloat factor, and the RPM headline number.
/// Averages across all rpm attempts that carry a result (typically one per
/// run iteration); loss/jitter come from the loaded phase — the user-felt
/// numbers when the link is loaded. "Jitter" here is the mean inter-probe
/// delay variation (IPDV), and "RPM" is our UDP-echo-under-load figure — NOT
/// a draft-ietf-ippm-responsiveness RPM (see `runner/rpm.rs` module docs).
/// When any loaded probe's echo wait was truncated before its full timeout
/// (`loaded_probes_censored`), a warning line flags the optimistic bias.
fn print_rpm_summary(run: &TestRun) {
    let results: Vec<&crate::metrics::RpmResult> = run
        .attempts
        .iter()
        .filter(|a| a.protocol == Protocol::Rpm)
        .filter_map(|a| a.rpm.as_ref())
        .collect();
    if results.is_empty() {
        return;
    }

    let avg = |f: &dyn Fn(&crate::metrics::RpmResult) -> f64| -> f64 {
        results.iter().map(|r| f(r)).sum::<f64>() / results.len() as f64
    };
    let avg_opt = |f: &dyn Fn(&crate::metrics::RpmResult) -> Option<f64>| -> Option<f64> {
        let vals: Vec<f64> = results.iter().filter_map(|r| f(r)).collect();
        (!vals.is_empty()).then(|| vals.iter().sum::<f64>() / vals.len() as f64)
    };

    println!();
    println!(
        " Latency under load (rpm, avg over {n} attempt{s})",
        n = results.len(),
        s = if results.len() == 1 { "" } else { "s" },
    );
    println!("──────────────────────────────────────────────────────────");
    println!("              │      Min │      Avg │      p95 │  Jitter │  Loss");
    println!(
        "   Unloaded   │ {min:>7.2}ms │ {a:>7.2}ms │ {p95:>7.2}ms │ {j:>6.2}ms │ {l:>4.1}%",
        min = avg(&|r| r.unloaded_rtt_min_ms),
        a = avg(&|r| r.unloaded_rtt_avg_ms),
        p95 = avg(&|r| r.unloaded_rtt_p95_ms),
        j = avg(&|r| r.unloaded_jitter_ms),
        l = avg(&|r| r.unloaded_loss_percent),
    );
    println!(
        "   Loaded     │ {min:>7.2}ms │ {a:>7.2}ms │ {p95:>7.2}ms │ {j:>6.2}ms │ {l:>4.1}%",
        min = avg(&|r| r.loaded_rtt_min_ms),
        a = avg(&|r| r.loaded_rtt_avg_ms),
        p95 = avg(&|r| r.loaded_rtt_p95_ms),
        j = avg(&|r| r.loaded_jitter_ms),
        l = avg(&|r| r.loaded_loss_percent),
    );
    let fmt =
        |v: Option<f64>, unit: &str| v.map_or_else(|| "—".to_string(), |x| format!("{x:.2}{unit}"));
    println!(
        "   → RPM: {rpm}  |  bufferbloat factor: {factor}  |  load: {mbps}",
        rpm = avg_opt(&|r| r.rpm)
            .map_or_else(|| "—".to_string(), |x| format!("{x:.0} round-trips/min")),
        factor = fmt(avg_opt(&|r| r.bufferbloat_factor), "x"),
        mbps = fmt(avg_opt(&|r| r.load_throughput_mbps), " MB/s"),
    );
    let censored: u32 = results
        .iter()
        .filter_map(|r| r.loaded_probes_censored)
        .sum();
    if censored > 0 {
        println!(
            "   ⚠ {censored} loaded probe(s) censored (echo wait truncated before \
             the full timeout) — loaded RTT/factor may be optimistically biased"
        );
    }
}

/// Render the draft-conformant responsiveness breakdown: per-direction RPM
/// (with the foreign/self split), capacity at saturation, connection count,
/// and the stability flags. Shows the FIRST attempt's result (typically one
/// per run iteration; per-attempt data is in the JSON output). Numbers here
/// follow draft-ietf-ippm-responsiveness-08 and ARE cross-tool comparable —
/// unlike the `rpm` mode's UDP-echo figure.
fn print_responsiveness_summary(run: &TestRun) {
    let Some(r) = run
        .attempts
        .iter()
        .filter(|a| a.protocol == Protocol::Responsiveness)
        .find_map(|a| a.responsiveness.as_ref())
    else {
        return;
    };

    let fmt =
        |v: Option<f64>, unit: &str| v.map_or_else(|| "—".to_string(), |x| format!("{x:.0}{unit}"));
    let fmt2 =
        |v: Option<f64>, unit: &str| v.map_or_else(|| "—".to_string(), |x| format!("{x:.2}{unit}"));

    println!();
    println!(" Responsiveness (draft-ietf-ippm-responsiveness)");
    println!("──────────────────────────────────────────────────────────");
    let render = |label: &str, d: &crate::metrics::ResponsivenessDirection| {
        println!(
            "   {label:<9} │ RPM {rpm:>6} (foreign {frpm}, self {srpm}) │ \
             capacity {cap} │ {conns} conns │ saturated: {sat}",
            rpm = fmt(d.rpm, ""),
            frpm = fmt(d.foreign_rpm, ""),
            srpm = fmt(d.self_rpm, ""),
            cap = fmt2(d.capacity_mbps, " MB/s"),
            conns = d.saturated_connections,
            sat = if d.saturation_reached {
                "yes"
            } else {
                "NO (cap hit)"
            },
        );
        println!(
            "             │ probe TM: tcp_f {tcp} tls_f {tls} http_f {httpf} http_l(self) {httpl} \
             │ probes ok {fok}/{fs} foreign, {sok}/{ss} self",
            tcp = fmt2(d.foreign_tcp_tm_ms, "ms"),
            tls = fmt2(d.foreign_tls_tm_ms, "ms"),
            httpf = fmt2(d.foreign_http_tm_ms, "ms"),
            httpl = fmt2(d.self_http_tm_ms, "ms"),
            fok = d.foreign_probes_ok,
            fs = d.foreign_probes_sent,
            sok = d.self_probes_ok,
            ss = d.self_probes_sent,
        );
    };
    render("Download", &r.download);
    match &r.upload {
        Some(u) => render("Upload", u),
        None => println!(
            "   Upload    │ absent — {}",
            r.upload_error.as_deref().unwrap_or("unknown reason")
        ),
    }
}

/// Render the multi-connection capacity breakdown: per-direction aggregate
/// capacity at saturation, connection count, fair-share spread, and the
/// per-connection goodput + TCP-attribution verdicts. Shows the FIRST
/// attempt's result; per-attempt data is in the JSON output.
fn print_mthroughput_summary(run: &TestRun) {
    let Some(m) = run
        .attempts
        .iter()
        .filter(|a| a.protocol == Protocol::Mthroughput)
        .find_map(|a| a.mthroughput.as_ref())
    else {
        return;
    };

    let fmt2 = |v: Option<f64>, unit: &str| {
        v.map_or_else(|| "\u{2014}".to_string(), |x| format!("{x:.2}{unit}"))
    };
    let fmt0 = |v: Option<f64>, unit: &str| {
        v.map_or_else(|| "\u{2014}".to_string(), |x| format!("{x:.0}{unit}"))
    };

    println!();
    println!(" Multi-connection throughput (link capacity)");
    println!("\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}");
    let render = |label: &str, d: &crate::metrics::MthroughputDirection| {
        println!(
            "   {label:<9} \u{2502} capacity {cap} \u{2502} {conns} conns \u{2502} saturated: {sat} \u{2502} \
             per-conn min {min} / mean {mean} / max {max} \u{2502} spread {spread}",
            cap = fmt2(d.capacity_mbps, " MB/s"),
            conns = d.connections,
            sat = if d.saturation_reached {
                "yes"
            } else {
                "NO (cap hit)"
            },
            min = fmt2(d.per_conn_min_mbps, ""),
            mean = fmt2(d.per_conn_mean_mbps, ""),
            max = fmt2(d.per_conn_max_mbps, ""),
            spread = fmt0(d.fair_share_spread_pct, "%"),
        );
        println!(
            "             \u{2502} attribution: {rw} rwnd-limited, {sb} sndbuf-limited, {pl} path-limited, \
             {un} unobserved",
            rw = d.rwnd_limited_conns,
            sb = d.sndbuf_limited_conns,
            pl = d.path_limited_conns,
            un = d.unobserved_conns,
        );
        for c in &d.per_conn {
            println!(
                "             \u{2502}   conn {idx}: {mbps:.2} MB/s \u{2502} {verdict} \u{2502} retrans {retrans}",
                idx = c.conn,
                mbps = c.mbps,
                verdict = c.verdict,
                retrans = c
                    .retrans
                    .map_or_else(|| "\u{2014}".to_string(), |r| r.to_string()),
            );
        }
    };
    render("Download", &m.download);
    match &m.upload {
        Some(u) => render("Upload", u),
        None => println!(
            "   Upload    \u{2502} absent \u{2014} {}",
            m.upload_error.as_deref().unwrap_or("unknown reason")
        ),
    }
}

/// Render the STAMP (RFC 8762) breakdown: processing-corrected RTT,
/// per-direction delay variation, and directional loss. Shows the FIRST
/// attempt's result; per-attempt data is in the JSON output.
fn print_stamp_summary(run: &TestRun) {
    let Some(s) = run
        .attempts
        .iter()
        .filter(|a| a.protocol == Protocol::Stamp)
        .find_map(|a| a.stamp.as_ref())
    else {
        return;
    };

    let fmt =
        |v: Option<f64>, unit: &str| v.map_or_else(|| "—".to_string(), |x| format!("{x:.2}{unit}"));
    println!();
    println!(" STAMP (RFC 8762, reflector {})", s.remote_addr);
    println!("──────────────────────────────────────────────────────────");
    println!(
        "   RTT (processing-corrected) │ min {min:.2}ms avg {avg:.2}ms p95 {p95:.2}ms │ \
         reflector processing avg {proc}",
        min = s.rtt_min_ms,
        avg = s.rtt_avg_ms,
        p95 = s.rtt_p95_ms,
        proc = s
            .reflector_processing_avg_us
            .map_or_else(|| "—".to_string(), |x| format!("{x:.0}µs")),
    );
    println!(
        "   Delay variation (IPDV)     │ near (out) mean {nm} p95 {np} │ far (back) mean {fm} p95 {fp}",
        nm = fmt(s.near_ipdv_mean_ms, "ms"),
        np = fmt(s.near_ipdv_p95_ms, "ms"),
        fm = fmt(s.far_ipdv_mean_ms, "ms"),
        fp = fmt(s.far_ipdv_p95_ms, "ms"),
    );
    println!(
        "   Loss                       │ sender→reflector {fwd} │ reflector→sender {rev} │ \
         replies {r}/{p}",
        fwd = fmt(s.loss_sent_percent, "%"),
        rev = fmt(s.loss_return_percent, "%"),
        r = s.replies_received,
        p = s.probes_sent,
    );
    if let (Some(f), Some(b), Some(u)) = (
        s.owd_forward_est_ms,
        s.owd_return_est_ms,
        s.owd_uncertainty_ms,
    ) {
        println!(
            "   One-way delay (ESTIMATE)   │ out {f:.2}ms │ back {b:.2}ms │ ±{u:.1}ms \
             (SNTP-offset-corrected; assumes reflector clock is NTP-true)"
        );
    }
}

/// Render the `path` hop table: per-hop router address + RTT on platforms
/// where hop addresses are observable unprivileged (Linux `IP_RECVERR`), or
/// the honest hop-count estimate + reachability line elsewhere. Shows the
/// FIRST attempt's trace (the path rarely changes between attempts of one
/// run; per-attempt data is in the JSON output).
fn print_path_summary(run: &TestRun) {
    let Some(p) = run
        .attempts
        .iter()
        .filter(|a| a.protocol == Protocol::Path)
        .find_map(|a| a.path.as_ref())
    else {
        return;
    };

    println!();
    println!(" Path to {} (method: {})", p.remote_addr, p.method);
    println!("──────────────────────────────────────────");
    for hop in &p.hops {
        let addr = hop.addr.as_deref().unwrap_or("*");
        let rtt = hop
            .rtt_ms
            .map(|r| format!("{r:>8.2}ms"))
            .unwrap_or_else(|| format!("{:>10}", "*"));
        println!("   {idx:>3}  {addr:<40} {rtt}", idx = hop.index);
    }
    if p.hops.is_empty() {
        println!("   (hop addresses not observable unprivileged on this platform)");
    }
    let hops = p
        .hop_count
        .map(|h| h.to_string())
        .unwrap_or_else(|| "unknown".into());
    let dest_rtt = p
        .destination_rtt_ms
        .map(|r| format!(", RTT {r:.2}ms"))
        .unwrap_or_default();
    println!(
        "   → hops: {hops}  |  destination {}{dest_rtt}",
        if p.destination_reached {
            "reached"
        } else {
            "NOT reached"
        },
    );
}

/// Render the `dualstack` IPv4-vs-IPv6 comparison: per-phase timing side by
/// side, which family was faster (+delta), and the happy-eyeballs verdict.
/// Averages phase timings across all dualstack attempts that carry a result.
fn print_dualstack_summary(run: &TestRun) {
    let results: Vec<&crate::metrics::DualStackResult> = run
        .attempts
        .iter()
        .filter(|a| a.protocol == Protocol::DualStack)
        .filter_map(|a| a.dualstack.as_ref())
        .collect();
    if results.is_empty() {
        return;
    }

    let avg = |f: &dyn Fn(&crate::metrics::DualStackResult) -> Option<f64>| -> Option<f64> {
        let vals: Vec<f64> = results.iter().filter_map(|r| f(r)).collect();
        (!vals.is_empty()).then(|| vals.iter().sum::<f64>() / vals.len() as f64)
    };
    let fmt = |v: Option<f64>| v.map_or_else(|| format!("{:>10}", "—"), |x| format!("{x:>8.2}ms"));
    let leg_status = |attempted: bool, success: bool| {
        if !attempted {
            "absent"
        } else if success {
            "ok"
        } else {
            "FAILED"
        }
    };

    // Status/verdict come from the first result (stable across attempts).
    let first = results[0];

    println!();
    println!(
        " Dual-stack comparison (avg over {n} attempt{s})",
        n = results.len(),
        s = if results.len() == 1 { "" } else { "s" },
    );
    println!("──────────────────────────────────────────────────────");
    println!(
        "              │       IPv4 │       IPv6\n   Status     │ {v4:>10} │ {v6:>10}",
        v4 = leg_status(first.ipv4.attempted, first.ipv4.success),
        v6 = leg_status(first.ipv6.attempted, first.ipv6.success),
    );
    let row = |label: &str,
               f4: &dyn Fn(&crate::metrics::DualStackResult) -> Option<f64>,
               f6: &dyn Fn(&crate::metrics::DualStackResult) -> Option<f64>| {
        println!(
            "   {label:<10} │ {v4:>10} │ {v6:>10}",
            v4 = fmt(avg(f4)),
            v6 = fmt(avg(f6)),
        );
    };
    row("DNS", &|r| r.ipv4.dns_ms, &|r| r.ipv6.dns_ms);
    row("TCP", &|r| r.ipv4.tcp_ms, &|r| r.ipv6.tcp_ms);
    row("TLS", &|r| r.ipv4.tls_ms, &|r| r.ipv6.tls_ms);
    row("TTFB", &|r| r.ipv4.ttfb_ms, &|r| r.ipv6.ttfb_ms);
    row("Total", &|r| r.ipv4.total_ms, &|r| r.ipv6.total_ms);
    match (&first.faster_family, avg(&|r| r.delta_ms)) {
        (Some(fam), Some(delta)) => {
            println!("   → faster family: {fam} by {delta:.2}ms avg");
        }
        _ => println!("   → no comparison (only one family completed)"),
    }
    println!("   → happy eyeballs: {}", first.happy_eyeballs_verdict);
}

/// Render per-record-type DNS depth for the standalone `dns` probe mode:
/// separately-timed A/AAAA lookups, record counts, and the CNAME chain.
/// Silent unless a dns-mode attempt captured the detail (older probes and all
/// other modes leave the fields unset).
fn print_dns_detail(run: &TestRun) {
    let dns: Vec<&crate::metrics::DnsResult> = run
        .attempts
        .iter()
        .filter(|a| a.protocol == Protocol::Dns)
        .filter_map(|a| a.dns.as_ref())
        .collect();
    let has_detail = dns
        .iter()
        .any(|d| d.a_ms.is_some() || d.aaaa_ms.is_some() || !d.cname_chain.is_empty());
    if !has_detail {
        return;
    }

    let avg = |f: &dyn Fn(&crate::metrics::DnsResult) -> Option<f64>| -> Option<f64> {
        let vals: Vec<f64> = dns.iter().filter_map(|d| f(d)).collect();
        (!vals.is_empty()).then(|| vals.iter().sum::<f64>() / vals.len() as f64)
    };
    let a_ms = avg(&|d| d.a_ms);
    let aaaa_ms = avg(&|d| d.aaaa_ms);

    // Record counts and the chain are stable across attempts — report the
    // first observation rather than a meaningless average.
    let a_count = dns.iter().find_map(|d| d.a_record_count);
    let aaaa_count = dns.iter().find_map(|d| d.aaaa_record_count);
    let chain = dns.iter().find(|d| !d.cname_chain.is_empty());

    let records = |n: Option<u32>| match n {
        Some(1) => "1 record".to_string(),
        Some(n) => format!("{n} records"),
        None => "skipped".to_string(),
    };
    let line = |label: &str, ms: Option<f64>, count: Option<u32>| match ms {
        Some(v) => println!("   {label:<18} {v:>8.2}ms avg │ {}", records(count)),
        None => println!("   {label:<18} {:>10} │ {}", "—", records(count)),
    };

    println!();
    println!(
        " DNS detail (over {n} probe{s})",
        n = dns.len(),
        s = if dns.len() == 1 { "" } else { "s" },
    );
    println!("──────────────────────────────────────────");
    line("A lookup", a_ms, a_count);
    line("AAAA lookup", aaaa_ms, aaaa_count);
    if let Some(d) = chain {
        println!(
            "   {label:<18} {query} → {chain}",
            label = "CNAME chain",
            query = d.query_name,
            chain = d.cname_chain.join(" → "),
        );
    }
}

/// Render certificate/OCSP depth for the standalone `tls` / `tlsresume`
/// modes: leaf key algorithm + size, signature algorithm, and whether the
/// server stapled an OCSP response. Silent when no attempt captured it.
fn print_tls_detail(run: &TestRun) {
    let Some(tls) = run
        .attempts
        .iter()
        .filter(|a| matches!(a.protocol, Protocol::Tls | Protocol::TlsResume))
        .filter_map(|a| a.tls.as_ref())
        .find(|t| {
            t.ocsp_stapled.is_some()
                || t.cert_chain
                    .first()
                    .is_some_and(|c| c.key_algorithm.is_some() || c.signature_algorithm.is_some())
        })
    else {
        return;
    };

    println!();
    println!(" TLS certificate detail");
    println!("──────────────────────────────────────────");
    if let Some(leaf) = tls.cert_chain.first() {
        if let Some(alg) = &leaf.key_algorithm {
            match leaf.key_size_bits {
                Some(bits) => println!("   {:<18} {alg} ({bits} bit)", "Leaf key"),
                None => println!("   {:<18} {alg}", "Leaf key"),
            }
        }
        if let Some(sig) = &leaf.signature_algorithm {
            println!("   {:<18} {sig}", "Signature");
        }
    }
    match tls.ocsp_stapled {
        Some(true) => println!(
            "   {:<18} stapled ({} bytes)",
            "OCSP",
            tls.ocsp_response_bytes.unwrap_or(0)
        ),
        Some(false) => println!("   {:<18} not stapled", "OCSP"),
        None => println!("   {:<18} not observed (resumed handshake)", "OCSP"),
    }
}

/// Render the HTTP/3 QUIC session-resumption / 0-RTT note: for each http3
/// attempt the probe opens a follow-up connection that tries to resume the
/// TLS 1.3 session and send the request in 0-RTT early data. This prints the
/// averaged cold-vs-resumed handshake comparison and the 0-RTT verdict.
fn print_h3_zero_rtt(run: &TestRun) {
    let h3: Vec<&RequestAttempt> = run
        .attempts
        .iter()
        .filter(|a| {
            a.protocol == Protocol::Http3
                && a.tls
                    .as_ref()
                    .is_some_and(|t| t.zero_rtt_attempted.is_some())
        })
        .collect();
    if h3.is_empty() {
        return;
    }

    let attempted = h3
        .iter()
        .filter(|a| {
            a.tls
                .as_ref()
                .is_some_and(|t| t.zero_rtt_attempted == Some(true))
        })
        .count();
    let accepted = h3
        .iter()
        .filter(|a| {
            a.tls
                .as_ref()
                .is_some_and(|t| t.zero_rtt_accepted == Some(true))
        })
        .count();

    let avg = |f: &dyn Fn(&RequestAttempt) -> Option<f64>| -> Option<f64> {
        let vals: Vec<f64> = h3.iter().filter_map(|a| f(a)).collect();
        if vals.is_empty() {
            None
        } else {
            Some(vals.iter().sum::<f64>() / vals.len() as f64)
        }
    };
    let full = avg(&|a| a.tls.as_ref().map(|t| t.handshake_duration_ms));
    let resumed = avg(&|a| a.tls.as_ref().and_then(|t| t.quic_resumed_handshake_ms));

    println!();
    println!(
        " QUIC 0-RTT (http3, {n} attempt{s}): 0-RTT attempted {attempted}/{n}, accepted {accepted}/{n}",
        n = h3.len(),
        s = if h3.len() == 1 { "" } else { "s" },
    );
    if let (Some(full), Some(resumed)) = (full, resumed) {
        let saved = full - resumed;
        let pct = if full > 0.0 {
            saved / full * 100.0
        } else {
            0.0
        };
        println!(
            "   handshake: full {full:.1}ms → resumed {resumed:.1}ms  (saved {saved:.1}ms, {pct:.0}%)"
        );
    }
}

pub fn print_comparison(run: &TestRun) {
    // Measured-phase attempts only, matching the benchmark JSON artifact.
    let measured = run.measured_attempts();
    let row = |proto: &Protocol| -> Option<String> {
        let attempts: Vec<&RequestAttempt> = measured
            .iter()
            .copied()
            .filter(|a| &a.protocol == proto)
            .collect();
        if attempts.is_empty() {
            return None;
        }
        let n = attempts.len();
        let pl_results: Vec<&PageLoadResult> = attempts
            .iter()
            .filter_map(|a| a.page_load.as_ref())
            .collect();
        if pl_results.is_empty() {
            return None;
        }
        let total_ms_vals: Vec<f64> = pl_results.iter().map(|p| p.total_ms).collect();
        let avg_conns: f64 = pl_results
            .iter()
            .map(|p| p.connections_opened as f64)
            .sum::<f64>()
            / n as f64;
        let avg_assets: f64 = pl_results
            .iter()
            .map(|p| p.assets_fetched as f64)
            .sum::<f64>()
            / n as f64;
        let total_assets = pl_results.first().map(|p| p.asset_count).unwrap_or(0);
        let avg_tls_ms: f64 = pl_results.iter().map(|p| p.tls_setup_ms).sum::<f64>() / n as f64;
        let avg_tls_pct: f64 = pl_results
            .iter()
            .map(|p| p.tls_overhead_ratio * 100.0)
            .sum::<f64>()
            / n as f64;
        let cpu_vals: Vec<f64> = pl_results.iter().filter_map(|p| p.cpu_time_ms).collect();
        let avg_cpu_str = if cpu_vals.is_empty() {
            "  —".into()
        } else {
            format!(
                "{:>5.1}",
                cpu_vals.iter().sum::<f64>() / cpu_vals.len() as f64
            )
        };
        let stats = compute_stats(&total_ms_vals)?;
        Some(format!(
            " {proto:<10} │ {n:<3} │ {assets:>3.0}/{total:<3} │ {conns:>5.1} │ {tls_ms:>8.1} │ {tls_pct:>6.1}% │ {cpu:>8} │ {p50:>8.1}ms │ {min:>8.1}ms │ {max:>8.1}ms",
            proto = proto,
            n = n,
            assets = avg_assets,
            total = total_assets,
            conns = avg_conns,
            tls_ms = avg_tls_ms,
            tls_pct = avg_tls_pct,
            cpu = avg_cpu_str,
            p50 = stats.p50,
            min = stats.min,
            max = stats.max,
        ))
    };

    // Browser row (uses BrowserResult, not PageLoadResult)
    let browser_row = |proto: &Protocol| -> Option<String> {
        let attempts: Vec<&RequestAttempt> = measured
            .iter()
            .copied()
            .filter(|a| &a.protocol == proto)
            .collect();
        if attempts.is_empty() {
            return None;
        }
        let n = attempts.len();
        let br_results: Vec<&BrowserResult> =
            attempts.iter().filter_map(|a| a.browser.as_ref()).collect();
        if br_results.is_empty() {
            return None;
        }
        let load_ms_vals: Vec<f64> = br_results.iter().map(|b| b.load_ms).collect();
        let avg_resources: f64 = br_results
            .iter()
            .map(|b| b.resource_count as f64)
            .sum::<f64>()
            / n as f64;
        let stats = compute_stats(&load_ms_vals)?;
        Some(format!(
            " {proto:<10} │ {n:<3} │ {res:>4.0}/—   │   —   │       —  │      —  │       —  │ {p50:>8.1}ms │ {min:>8.1}ms │ {max:>8.1}ms",
            proto = proto,
            n = n,
            res = avg_resources,
            p50 = stats.p50,
            min = stats.min,
            max = stats.max,
        ))
    };

    println!();
    println!(" ── Protocol Comparison (Page Load) ─────────────────────────────────────────────────────────────────────────");
    println!(" Protocol  │ N   │ Assets  │ Conns │  TLS ms  │  TLS %  │  CPU ms  │   p50    │   Min    │   Max");
    println!("───────────┼─────┼─────────┼───────┼──────────┼─────────┼──────────┼──────────┼──────────┼──────────");
    for proto in &[Protocol::PageLoad, Protocol::PageLoad2, Protocol::PageLoad3] {
        if let Some(r) = row(proto) {
            println!("{r}");
        }
    }
    for proto in &[
        Protocol::Browser,
        Protocol::Browser1,
        Protocol::Browser2,
        Protocol::Browser3,
    ] {
        if let Some(r) = browser_row(proto) {
            println!("{r}");
        }
    }
}

pub fn print_url_test_summary(run: &UrlTestRun, json_path: &Path) {
    println!("URL Test Summary");
    println!("----------------");
    println!("Requested URL: {}", run.requested_url);
    if let Some(final_url) = &run.final_url {
        println!("Final URL: {final_url}");
    }
    println!("Status: {:?}", run.status);
    println!();
    println!("Primary Load");
    println!(
        "- Observed Protocol (main document): {}",
        run.observed_protocol_primary_load
            .as_deref()
            .unwrap_or("unknown")
    );
    println!(
        "- Primary Origin: {}",
        run.primary_origin.as_deref().unwrap_or("-")
    );
    println!();
    println!("Milestones");
    println!(
        "- DNS: {}",
        run.dns_ms
            .map(|v| format!("{v:.0} ms"))
            .unwrap_or_else(|| "-".into())
    );
    println!(
        "- Connect: {}",
        run.connect_ms
            .map(|v| format!("{v:.0} ms"))
            .unwrap_or_else(|| "-".into())
    );
    println!(
        "- Handshake: {}",
        run.handshake_ms
            .map(|v| format!("{v:.0} ms"))
            .unwrap_or_else(|| "-".into())
    );
    println!(
        "- TTFB: {}",
        run.ttfb_ms
            .map(|v| format!("{v:.0} ms"))
            .unwrap_or_else(|| "-".into())
    );
    println!(
        "- DOMContentLoaded: {}",
        run.dom_content_loaded_ms
            .map(|v| format!("{v:.0} ms"))
            .unwrap_or_else(|| "-".into())
    );
    println!(
        "- Load Event: {}",
        run.load_event_ms
            .map(|v| format!("{v:.0} ms"))
            .unwrap_or_else(|| "-".into())
    );
    println!();
    println!("Page Summary");
    println!("- Requests: {}", run.total_requests);
    println!("- Transfer Size: {} bytes", run.total_transfer_bytes);
    println!("- Failures: {}", run.failure_count);
    println!();
    // Security-header audit (derived from the protocol probes' captured
    // response headers; omitted entirely when no probe captured headers).
    if let Some(sec) = &run.security_headers {
        println!("Security Headers");
        println!(
            "- HSTS: {}",
            sec.hsts.as_deref().map_or_else(
                || "absent".to_string(),
                |v| match sec.hsts_max_age_secs {
                    Some(age) => format!("{v} (max-age {age}s)"),
                    None => v.to_string(),
                }
            )
        );
        let yes_no = |v: Option<bool>| match v {
            Some(true) => "yes",
            Some(false) => "no",
            None => "-",
        };
        println!("- CSP present: {}", yes_no(sec.csp_present));
        println!(
            "- X-Content-Type-Options nosniff: {}",
            yes_no(sec.x_content_type_options_nosniff)
        );
        println!(
            "- X-Frame-Options: {}",
            sec.x_frame_options.as_deref().unwrap_or("absent")
        );
        println!(
            "- Referrer-Policy: {}",
            sec.referrer_policy.as_deref().unwrap_or("absent")
        );
        println!();
    }
    if !run.capture_errors.is_empty() {
        println!("Warnings");
        for err in &run.capture_errors {
            println!("- {err}");
        }
        println!();
    }
    println!("Artifacts");
    println!("- JSON: {}", json_path.display());
    println!(
        "- HAR: {}",
        run.har_path.as_deref().unwrap_or("not captured")
    );
    println!(
        "- PCAP: {}",
        run.pcap_path.as_deref().unwrap_or("not captured")
    );
}

/// Copy the bundled `report.css` from the binary's embedded bytes to the
/// output directory so the HTML report can link to it.
pub fn copy_default_css(out_dir: &Path) {
    let dest = out_dir.join("report.css");
    if dest.exists() {
        return;
    }
    if let Ok(src) = std::fs::read("assets/report.css") {
        let _ = std::fs::write(&dest, src);
    } else {
        let _ = std::fs::write(&dest, crate::output::html::FALLBACK_CSS);
    }
}

/// Render the `websocket` breakdown: connection ladder (DNS/TCP/TLS), the
/// one-time HTTP 101 upgrade cost, and the steady-state message-RTT
/// distribution. Averages across all websocket attempts that carry a result.
fn print_websocket_summary(run: &TestRun) {
    let attempts: Vec<&RequestAttempt> = run
        .attempts
        .iter()
        .filter(|a| a.protocol == Protocol::WebSocket && a.websocket.is_some())
        .collect();
    if attempts.is_empty() {
        return;
    }

    let avg = |f: &dyn Fn(&RequestAttempt) -> Option<f64>| -> Option<f64> {
        let vals: Vec<f64> = attempts.iter().filter_map(|a| f(a)).collect();
        (!vals.is_empty()).then(|| vals.iter().sum::<f64>() / vals.len() as f64)
    };
    let ws = |f: &dyn Fn(&crate::metrics::WebSocketResult) -> Option<f64>| -> Option<f64> {
        avg(&|a| a.websocket.as_ref().and_then(f))
    };
    let fmt = |v: Option<f64>| v.map_or_else(|| "—".to_string(), |x| format!("{x:.2}ms"));

    let echoes: u32 = attempts
        .iter()
        .filter_map(|a| a.websocket.as_ref())
        .map(|w| w.echo_count)
        .sum();
    let sent: u32 = attempts
        .iter()
        .filter_map(|a| a.websocket.as_ref())
        .map(|w| w.message_count)
        .sum();

    println!();
    println!(
        " WebSocket (avg over {n} attempt{s})",
        n = attempts.len(),
        s = if attempts.len() == 1 { "" } else { "s" },
    );
    println!("──────────────────────────────────────────");
    let line = |label: &str, v: Option<f64>| {
        println!("   {label:<16} {val:>12}", val = fmt(v));
    };
    line("DNS", avg(&|a| a.dns.as_ref().map(|d| d.duration_ms)));
    line(
        "TCP connect",
        avg(&|a| a.tcp.as_ref().map(|t| t.connect_duration_ms)),
    );
    line(
        "TLS handshake",
        avg(&|a| a.tls.as_ref().map(|t| t.handshake_duration_ms)),
    );
    line("Upgrade (101)", ws(&|w| Some(w.upgrade_ms)));
    // Message RTTs only over attempts that actually received echoes — a
    // fully-lost attempt's 0.0 sentinel is not a measurement (V11).
    let echoed = |f: &dyn Fn(&crate::metrics::WebSocketResult) -> f64| -> Option<f64> {
        ws(&|w| (w.echo_count > 0).then(|| f(w)))
    };
    line("Msg RTT min", echoed(&|w| w.msg_rtt_min_ms));
    line("Msg RTT avg", echoed(&|w| w.msg_rtt_avg_ms));
    line("Msg RTT p95", echoed(&|w| w.msg_rtt_p95_ms));
    line("Msg jitter", echoed(&|w| w.jitter_ms));
    println!(
        "   → echoes: {echoes}/{sent} ({loss:.1}% loss)",
        loss = if sent > 0 {
            100.0 * (sent - echoes) as f64 / sent as f64
        } else {
            0.0
        },
    );
}

/// Render the `pmtud` verdict: discovered path MTU (flagged when it is only a
/// lower bound), the ICMP-reported next-hop MTU, the local interface MTU for
/// contrast, and the method that produced the number. Shows the FIRST
/// attempt's result (the path MTU rarely changes between attempts of one
/// run; per-attempt data is in the JSON output).
fn print_pmtud_summary(run: &TestRun) {
    let Some(p) = run
        .attempts
        .iter()
        .filter(|a| a.protocol == Protocol::Pmtud)
        .find_map(|a| a.pmtud.as_ref())
    else {
        return;
    };

    println!();
    println!(" Path MTU to {} (method: {})", p.remote_addr, p.method);
    println!("──────────────────────────────────────────");
    match (p.path_mtu, p.lower_bound_only) {
        (Some(mtu), false) => {
            println!(
                "   path MTU: {mtu} bytes (max unfragmented payload {payload} + {hdr} header)",
                payload = p.max_unfragmented_payload.unwrap_or(0),
                hdr = p.header_bytes,
            );
        }
        (Some(mtu), true) => {
            println!(
                "   path MTU: ≥{mtu} bytes (search ceiling fit unfragmented — true MTU may be higher)"
            );
        }
        (None, _) => {
            println!("   path MTU: unknown — no echo, no ICMP, no send errors (black hole or silent path)");
        }
    }
    if let Some(m) = p.icmp_mtu {
        println!("   ICMP next-hop MTU: {m} bytes");
    }
    if let Some(m) = p.local_mtu {
        let note = match p.path_mtu {
            Some(pm) if pm < m => " — path is narrower than the local link",
            _ => "",
        };
        println!("   local interface MTU: {m} bytes{note}");
    }
    println!("   → {probes} DF probe(s) sent", probes = p.probes_sent);
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{attribution_note, local_drop_note};

    // ── Throughput-attribution triad (B.2) ───────────────────────────────────

    #[test]
    fn attribution_note_none_without_triad_data() {
        assert!(attribution_note(&[]).is_none());
    }

    #[test]
    fn attribution_note_quiet_when_path_limited() {
        // 2% rwnd, 1% sndbuf — normal path/CPU-limited transfer stays silent.
        let line = attribution_note(&[(1_000_000, 20_000, 10_000)]);
        assert!(line.is_none(), "unexpected note: {line:?}");
    }

    #[test]
    fn attribution_note_reports_rwnd_limited_share() {
        let line = attribution_note(&[(1_000_000, 840_000, 0)]).expect("note expected");
        assert!(
            line.contains("receiver-window-limited 84% of the transfer"),
            "{line}"
        );
        assert!(!line.contains("send-buffer-limited"), "{line}");
        assert!(line.contains("1 attempt"), "{line}");
    }

    #[test]
    fn attribution_note_reports_sndbuf_limited_share() {
        let line = attribution_note(&[(2_000_000, 0, 500_000)]).expect("note expected");
        assert!(
            line.contains("local send-buffer-limited 25% of the transfer"),
            "{line}"
        );
        assert!(!line.contains("receiver-window-limited"), "{line}");
    }

    #[test]
    fn attribution_note_aggregates_across_attempts() {
        // Two attempts: 50% and 100% rwnd-limited → 75% of total busy time.
        let line = attribution_note(&[(1_000_000, 500_000, 0), (1_000_000, 1_000_000, 0)])
            .expect("note expected");
        assert!(line.contains("receiver-window-limited 75%"), "{line}");
        assert!(line.contains("2 attempts"), "{line}");
    }

    #[test]
    fn attribution_note_reports_both_limits_when_both_noteworthy() {
        let line = attribution_note(&[(1_000_000, 300_000, 200_000)]).expect("note expected");
        assert!(line.contains("receiver-window-limited 30%"), "{line}");
        assert!(line.contains("local send-buffer-limited 20%"), "{line}");
    }

    // ── UDP local-drop warning (B.6) ─────────────────────────────────────────

    #[test]
    fn local_drop_note_none_when_no_drops_observed() {
        assert!(local_drop_note(&[]).is_none());
        assert!(local_drop_note(&[0, 0, 0]).is_none());
    }

    #[test]
    fn local_drop_note_reports_count_and_honest_attribution() {
        let line = local_drop_note(&[0, 12, 0]).expect("note expected");
        assert!(line.contains("12 datagrams dropped locally"), "{line}");
        assert!(line.contains("1 attempt "), "{line}");
        assert!(line.contains("loss% includes them"), "{line}");
        assert!(line.contains("not path loss"), "{line}");
    }

    #[test]
    fn local_drop_note_singular_forms() {
        let line = local_drop_note(&[1]).expect("note expected");
        assert!(line.contains("1 datagram dropped locally"), "{line}");
    }

    #[test]
    fn local_drop_note_sums_across_attempts() {
        let line = local_drop_note(&[3, 4]).expect("note expected");
        assert!(line.contains("7 datagrams"), "{line}");
        assert!(line.contains("2 attempts"), "{line}");
    }
}
