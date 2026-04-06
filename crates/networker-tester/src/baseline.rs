use crate::metrics::{
    BenchmarkEnvironmentCheck, BenchmarkStabilityCheck, HostInfo, NetworkBaseline, NetworkType,
};

pub const DEFAULT_ENVIRONMENT_CHECK_SAMPLES: u32 = 5;
pub const DEFAULT_ENVIRONMENT_CHECK_INTERVAL_MS: u64 = 50;
pub const DEFAULT_STABILITY_CHECK_SAMPLES: u32 = 12;
pub const DEFAULT_STABILITY_CHECK_INTERVAL_MS: u64 = 50;

/// Classify an IP address as Loopback, LAN (private), or Internet (public).
pub fn classify_ip(ip: &std::net::IpAddr) -> NetworkType {
    match ip {
        std::net::IpAddr::V4(v4) => {
            if v4.is_loopback() {
                NetworkType::Loopback
            } else if v4.is_private()
                || v4.is_link_local()
                || v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64
            {
                // 10.x, 172.16-31.x, 192.168.x, 169.254.x, 100.64-127.x (CGNAT)
                NetworkType::LAN
            } else {
                NetworkType::Internet
            }
        }
        std::net::IpAddr::V6(v6) => {
            if v6.is_loopback() {
                NetworkType::Loopback
            } else {
                let segs = v6.segments();
                if segs[0] == 0xfe80 || segs[0] & 0xfe00 == 0xfc00 {
                    // Link-local (fe80::) or ULA (fc00::/7)
                    NetworkType::LAN
                } else {
                    NetworkType::Internet
                }
            }
        }
    }
}

/// Classify the network type based on the target hostname/IP.
pub fn classify_target(host: &str) -> NetworkType {
    if host == "localhost" {
        return NetworkType::Loopback;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return classify_ip(&ip);
    }
    // For hostnames, try DNS resolution and classify the first IP
    use std::net::ToSocketAddrs;
    if let Ok(mut addrs) = (host, 0u16).to_socket_addrs() {
        if let Some(addr) = addrs.next() {
            return classify_ip(&addr.ip());
        }
    }
    NetworkType::Internet // default for unresolvable hostnames
}

/// Measure TCP connect RTT to a target N times (returns sorted RTTs in ms).
pub async fn measure_rtt(host: &str, port: u16, samples: u32) -> Vec<f64> {
    measure_rtt_samples(host, port, samples, DEFAULT_STABILITY_CHECK_INTERVAL_MS)
        .await
        .0
}

/// Measure TCP connect RTT to a target N times, preserving attempt order.
pub async fn measure_rtt_samples(
    host: &str,
    port: u16,
    samples: u32,
    interval_ms: u64,
) -> (Vec<f64>, u32, f64) {
    let mut rtts = Vec::with_capacity(samples as usize);
    let addr = format!("{host}:{port}");
    let started = std::time::Instant::now();
    for _ in 0..samples {
        let t0 = std::time::Instant::now();
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::net::TcpStream::connect(&addr),
        )
        .await
        {
            Ok(Ok(_stream)) => {
                rtts.push(t0.elapsed().as_secs_f64() * 1000.0);
            }
            _ => {
                // Connection failed or timed out; skip this sample
            }
        }
        if interval_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
        }
    }
    (rtts, samples, started.elapsed().as_secs_f64() * 1000.0)
}

/// Compute a percentile from a sorted slice (linear interpolation).
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let idx = p / 100.0 * (sorted.len() - 1) as f64;
    let lo = idx.floor() as usize;
    let hi = idx.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        sorted[lo] + (sorted[hi] - sorted[lo]) * (idx - lo as f64)
    }
}

/// Run a network baseline measurement: TCP RTT probes + network classification.
/// Always returns the network type (LAN/Internet/Loopback) even if RTT probes fail,
/// so that LAN targets are correctly identified as reference-only in the report.
pub async fn measure_baseline(target: &url::Url) -> Option<NetworkBaseline> {
    let host = target.host_str()?;
    let port = target.port_or_known_default()?;
    let network_type = classify_target(host);

    let rtts = measure_rtt(host, port, 5).await;
    if rtts.is_empty() {
        // RTT probes failed (target unreachable) but we still know the network type
        return Some(NetworkBaseline {
            samples: 0,
            rtt_min_ms: 0.0,
            rtt_avg_ms: 0.0,
            rtt_max_ms: 0.0,
            rtt_p50_ms: 0.0,
            rtt_p95_ms: 0.0,
            network_type,
        });
    }

    let sum: f64 = rtts.iter().sum();
    Some(NetworkBaseline {
        samples: rtts.len() as u32,
        rtt_min_ms: rtts[0],
        rtt_avg_ms: sum / rtts.len() as f64,
        rtt_max_ms: rtts[rtts.len() - 1],
        rtt_p50_ms: percentile(&rtts, 50.0),
        rtt_p95_ms: percentile(&rtts, 95.0),
        network_type,
    })
}

pub fn baseline_from_environment_check(
    environment_check: &BenchmarkEnvironmentCheck,
) -> NetworkBaseline {
    NetworkBaseline {
        samples: environment_check.successful_samples,
        rtt_min_ms: environment_check.rtt_min_ms,
        rtt_avg_ms: environment_check.rtt_avg_ms,
        rtt_max_ms: environment_check.rtt_max_ms,
        rtt_p50_ms: environment_check.rtt_p50_ms,
        rtt_p95_ms: environment_check.rtt_p95_ms,
        network_type: environment_check.network_type,
    }
}

pub fn average_jitter_ms(samples: &[f64]) -> f64 {
    if samples.len() < 2 {
        return 0.0;
    }
    samples
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .sum::<f64>()
        / (samples.len() - 1) as f64
}

pub fn baseline_from_stability_check(stability_check: &BenchmarkStabilityCheck) -> NetworkBaseline {
    NetworkBaseline {
        samples: stability_check.successful_samples,
        rtt_min_ms: stability_check.rtt_min_ms,
        rtt_avg_ms: stability_check.rtt_avg_ms,
        rtt_max_ms: stability_check.rtt_max_ms,
        rtt_p50_ms: stability_check.rtt_p50_ms,
        rtt_p95_ms: stability_check.rtt_p95_ms,
        network_type: stability_check.network_type,
    }
}

pub async fn measure_environment_check(
    target: &url::Url,
    samples: u32,
    interval_ms: u64,
) -> Option<BenchmarkEnvironmentCheck> {
    let host = target.host_str()?;
    let port = target.port_or_known_default()?;
    let network_type = classify_target(host);
    let (ordered_rtts, attempted_samples, duration_ms) =
        measure_rtt_samples(host, port, samples, interval_ms).await;
    let successful_samples = ordered_rtts.len() as u32;
    let failed_samples = attempted_samples.saturating_sub(successful_samples);
    let packet_loss_percent = if attempted_samples > 0 {
        failed_samples as f64 / attempted_samples as f64 * 100.0
    } else {
        0.0
    };
    let mut sorted_rtts = ordered_rtts.clone();
    sorted_rtts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    if sorted_rtts.is_empty() {
        return Some(BenchmarkEnvironmentCheck {
            attempted_samples,
            successful_samples,
            failed_samples,
            duration_ms,
            rtt_min_ms: 0.0,
            rtt_avg_ms: 0.0,
            rtt_max_ms: 0.0,
            rtt_p50_ms: 0.0,
            rtt_p95_ms: 0.0,
            packet_loss_percent,
            network_type,
        });
    }

    let sum: f64 = sorted_rtts.iter().sum();
    Some(BenchmarkEnvironmentCheck {
        attempted_samples,
        successful_samples,
        failed_samples,
        duration_ms,
        rtt_min_ms: sorted_rtts[0],
        rtt_avg_ms: sum / sorted_rtts.len() as f64,
        rtt_max_ms: sorted_rtts[sorted_rtts.len() - 1],
        rtt_p50_ms: percentile(&sorted_rtts, 50.0),
        rtt_p95_ms: percentile(&sorted_rtts, 95.0),
        packet_loss_percent,
        network_type,
    })
}

pub async fn measure_stability_check(
    target: &url::Url,
    samples: u32,
    interval_ms: u64,
) -> Option<BenchmarkStabilityCheck> {
    let host = target.host_str()?;
    let port = target.port_or_known_default()?;
    let network_type = classify_target(host);
    let (ordered_rtts, attempted_samples, duration_ms) =
        measure_rtt_samples(host, port, samples, interval_ms).await;
    let successful_samples = ordered_rtts.len() as u32;
    let failed_samples = attempted_samples.saturating_sub(successful_samples);
    let packet_loss_percent = if attempted_samples > 0 {
        failed_samples as f64 / attempted_samples as f64 * 100.0
    } else {
        0.0
    };
    let jitter_ms = average_jitter_ms(&ordered_rtts);
    let mut sorted_rtts = ordered_rtts.clone();
    sorted_rtts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    if sorted_rtts.is_empty() {
        return Some(BenchmarkStabilityCheck {
            attempted_samples,
            successful_samples,
            failed_samples,
            duration_ms,
            rtt_min_ms: 0.0,
            rtt_avg_ms: 0.0,
            rtt_max_ms: 0.0,
            rtt_p50_ms: 0.0,
            rtt_p95_ms: 0.0,
            jitter_ms,
            packet_loss_percent,
            network_type,
        });
    }

    let sum: f64 = sorted_rtts.iter().sum();
    Some(BenchmarkStabilityCheck {
        attempted_samples,
        successful_samples,
        failed_samples,
        duration_ms,
        rtt_min_ms: sorted_rtts[0],
        rtt_avg_ms: sum / sorted_rtts.len() as f64,
        rtt_max_ms: *sorted_rtts.last().unwrap_or(&sorted_rtts[0]),
        rtt_p50_ms: percentile(&sorted_rtts, 50.0),
        rtt_p95_ms: percentile(&sorted_rtts, 95.0),
        jitter_ms,
        packet_loss_percent,
        network_type,
    })
}

/// Fetch server metadata from GET /info before probes begin.
pub async fn fetch_server_info(target: &url::Url, insecure: bool) -> Option<HostInfo> {
    let info_url = {
        let mut u = target.clone();
        u.set_path("/info");
        u.set_query(None);
        u.to_string()
    };

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(insecure)
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;

    let resp = client.get(&info_url).send().await.ok()?;
    let json: serde_json::Value = resp.json().await.ok()?;

    let sys = json.get("system")?;
    Some(HostInfo {
        os: sys.get("os")?.as_str()?.to_string(),
        arch: sys.get("arch")?.as_str()?.to_string(),
        cpu_cores: sys.get("cpu_cores")?.as_u64()? as usize,
        total_memory_mb: sys.get("total_memory_mb").and_then(|v| v.as_u64()),
        os_version: sys
            .get("os_version")
            .and_then(|v| v.as_str())
            .map(String::from),
        hostname: sys
            .get("hostname")
            .and_then(|v| v.as_str())
            .map(String::from),
        server_version: json
            .get("version")
            .and_then(|v| v.as_str())
            .map(String::from),
        uptime_secs: json.get("uptime_secs").and_then(|v| v.as_u64()),
        region: json
            .get("region")
            .and_then(|v| v.as_str())
            .map(String::from),
    })
}
