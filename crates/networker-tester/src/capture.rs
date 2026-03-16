use anyhow::Context;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use tokio::task::spawn_blocking;
use tokio::time::{sleep, Duration};

use crate::cli::{PacketCaptureMode, ResolvedConfig};

#[derive(Debug, Clone)]
pub struct PacketCapturePlan {
    pub mode: PacketCaptureMode,
    pub interface: String,
    pub write_pcap: bool,
    pub write_summary_json: bool,
    pub pcap_path: PathBuf,
    pub summary_path: PathBuf,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct PacketShare {
    pub protocol: String,
    pub packets: u64,
    pub pct_of_total: f64,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct EndpointPacketCount {
    pub endpoint: String,
    pub packets: u64,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct PortPacketCount {
    pub port: u16,
    pub packets: u64,
}

#[derive(Debug, Serialize)]
pub struct PacketCaptureSummary {
    pub mode: String,
    pub interface: String,
    pub capture_path: String,
    pub tshark_path: String,
    pub total_packets: u64,
    pub capture_status: String,
    pub note: Option<String>,
    pub warnings: Vec<String>,
    pub likely_target_endpoints: Vec<String>,
    pub tcp_packets: u64,
    pub udp_packets: u64,
    pub quic_packets: u64,
    pub http_packets: u64,
    pub dns_packets: u64,
    pub retransmissions: u64,
    pub duplicate_acks: u64,
    pub resets: u64,
    pub transport_shares: Vec<PacketShare>,
    pub top_endpoints: Vec<EndpointPacketCount>,
    pub top_ports: Vec<PortPacketCount>,
    pub observed_quic: bool,
    pub observed_tcp_only: bool,
    pub observed_mixed_transport: bool,
    pub capture_may_be_ambiguous: bool,
}

#[derive(Debug)]
pub struct PacketCaptureSession {
    child: Child,
    plan: PacketCapturePlan,
    tshark_path: PathBuf,
    stderr_path: PathBuf,
}

pub fn build_plan(cfg: &ResolvedConfig, out_dir: &Path) -> Option<PacketCapturePlan> {
    let pc = &cfg.packet_capture;
    if !pc.mode.captures_tester() {
        return None;
    }

    Some(PacketCapturePlan {
        mode: pc.mode,
        interface: resolve_capture_interface(pc.interface.as_str(), &cfg.targets),
        write_pcap: pc.write_pcap,
        write_summary_json: pc.write_summary_json,
        pcap_path: out_dir.join("packet-capture-tester.pcapng"),
        summary_path: out_dir.join("packet-capture-summary.json"),
    })
}

pub fn detect_tshark() -> Option<PathBuf> {
    [
        "tshark",
        "/opt/homebrew/bin/tshark",
        "/usr/local/bin/tshark",
    ]
    .into_iter()
    .find_map(which)
}

fn resolve_capture_interface(requested: &str, targets: &[String]) -> String {
    if requested != "auto" {
        return requested.to_string();
    }

    let localhost = targets.iter().all(|target| {
        target.contains("127.0.0.1") || target.contains("localhost") || target.contains("[::1]")
    });

    if localhost {
        #[cfg(target_os = "macos")]
        {
            return "lo0".into();
        }
        #[cfg(target_os = "linux")]
        {
            return "lo".into();
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            return "auto".into();
        }
    }

    #[cfg(target_os = "linux")]
    {
        "any".into()
    }
    #[cfg(target_os = "macos")]
    {
        "en0".into()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        "auto".into()
    }
}

fn check_capture_prereqs_blocking(
    plan: &PacketCapturePlan,
    tshark_path: &Path,
) -> anyhow::Result<()> {
    #[cfg(not(target_os = "macos"))]
    let _ = plan;

    let out = Command::new(tshark_path)
        .arg("-D")
        .output()
        .context("run tshark -D")?;
    if !out.status.success() {
        anyhow::bail!("tshark is installed but interface listing failed")
    }

    #[cfg(target_os = "macos")]
    {
        let probe = Command::new(tshark_path)
            .arg("-i")
            .arg(&plan.interface)
            .arg("-a")
            .arg("duration:1")
            .arg("-w")
            .arg("/dev/null")
            .output()
            .context("run macOS packet-capture permission probe")?;
        let stderr = String::from_utf8_lossy(&probe.stderr);
        if stderr.contains("Permission denied") || stderr.contains("cannot open BPF device") {
            anyhow::bail!(
                "packet capture permission denied on interface {}. Install/enable ChmodBPF for Wireshark/TShark on macOS",
                plan.interface
            );
        }
    }

    Ok(())
}

pub async fn check_capture_prereqs(
    plan: &PacketCapturePlan,
    tshark_path: &Path,
) -> anyhow::Result<()> {
    let plan = plan.clone();
    let tshark_path = tshark_path.to_path_buf();
    spawn_blocking(move || check_capture_prereqs_blocking(&plan, &tshark_path))
        .await
        .context("join packet capture prereq task")?
}

pub async fn start(plan: PacketCapturePlan) -> anyhow::Result<PacketCaptureSession> {
    let tshark_path =
        detect_tshark().context("packet capture requested but tshark was not found")?;
    check_capture_prereqs(&plan, &tshark_path).await?;
    let stderr_path = plan
        .summary_path
        .with_file_name("packet-capture-tshark.stderr.log");
    let stderr_file = std::fs::File::create(&stderr_path)
        .with_context(|| format!("create {}", stderr_path.display()))?;

    let mut cmd = Command::new(&tshark_path);
    cmd.arg("-q")
        .arg("-w")
        .arg(&plan.pcap_path)
        .arg("-i")
        .arg(&plan.interface)
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file));

    let child = cmd.spawn().context("failed to start tshark capture")?;
    sleep(Duration::from_millis(1000)).await;

    Ok(PacketCaptureSession {
        child,
        plan,
        tshark_path,
        stderr_path,
    })
}

impl PacketCaptureSession {
    pub async fn finalize(mut self) -> anyhow::Result<Option<PacketCaptureSummary>> {
        if self.child.try_wait()?.is_none() {
            #[cfg(unix)]
            unsafe {
                // SAFETY: `self.child.id()` comes from a live child process handle we own.
                // We only send SIGINT after `try_wait()` confirms it has not exited yet.
                let _ = libc::kill(self.child.id() as libc::pid_t, libc::SIGINT);
            }
            #[cfg(windows)]
            {
                tracing::warn!(
                    "graceful tshark shutdown is not implemented on Windows yet; skipping packet capture finalize for this run"
                );
                let _ = self.child.kill();
            }
        }
        // Give tshark time to flush pcap buffers after the graceful interrupt.
        sleep(Duration::from_millis(1200)).await;
        if self.child.try_wait()?.is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        // Wait a moment for the capture file to become visible before summarizing it.
        sleep(Duration::from_millis(400)).await;

        if !self.plan.write_summary_json {
            return Ok(None);
        }

        if !self.plan.pcap_path.exists() {
            anyhow::bail!(
                "packet capture file was not created: {} (stderr: {})",
                self.plan.pcap_path.display(),
                self.stderr_path.display()
            );
        }

        let tshark_path = self.tshark_path.clone();
        let plan = self.plan.clone();
        let targets = vec![];
        let summary = spawn_blocking(move || summarize(&tshark_path, &plan, &targets))
            .await
            .context("join packet capture summary task")??;
        std::fs::write(
            &self.plan.summary_path,
            serde_json::to_vec_pretty(&summary).context("serialize packet capture summary")?,
        )
        .with_context(|| format!("write {}", self.plan.summary_path.display()))?;

        if !self.plan.write_pcap {
            let _ = std::fs::remove_file(&self.plan.pcap_path);
        }

        Ok(Some(summary))
    }
}

fn summarize(
    tshark_path: &Path,
    plan: &PacketCapturePlan,
    targets: &[String],
) -> anyhow::Result<PacketCaptureSummary> {
    // TODO: batch these tshark reads so large pcaps are not re-read once per filter.
    let stats = [
        ("tcp", "tcp_packets"),
        ("udp", "udp_packets"),
        ("quic", "quic_packets"),
        ("http", "http_packets"),
        ("dns", "dns_packets"),
        ("tcp.analysis.retransmission", "retransmissions"),
        ("tcp.analysis.duplicate_ack", "duplicate_acks"),
        ("tcp.flags.reset == 1", "resets"),
    ];

    let total_packets = count_matches(tshark_path, &plan.pcap_path, "frame").unwrap_or(0);
    let mut summary = PacketCaptureSummary {
        mode: format!("{:?}", plan.mode).to_lowercase(),
        interface: plan.interface.clone(),
        capture_path: plan.pcap_path.display().to_string(),
        tshark_path: tshark_path.display().to_string(),
        total_packets,
        capture_status: if total_packets > 0 {
            "captured".into()
        } else {
            "empty".into()
        },
        note: None,
        warnings: vec![],
        likely_target_endpoints: vec![],
        tcp_packets: 0,
        udp_packets: 0,
        quic_packets: 0,
        http_packets: 0,
        dns_packets: 0,
        retransmissions: 0,
        duplicate_acks: 0,
        resets: 0,
        transport_shares: vec![],
        top_endpoints: vec![],
        top_ports: vec![],
        observed_quic: false,
        observed_tcp_only: false,
        observed_mixed_transport: false,
        capture_may_be_ambiguous: false,
    };

    for (filter, field) in stats {
        let count = count_matches(tshark_path, &plan.pcap_path, filter).unwrap_or(0);
        match field {
            "tcp_packets" => summary.tcp_packets = count,
            "udp_packets" => summary.udp_packets = count,
            "quic_packets" => summary.quic_packets = count,
            "http_packets" => summary.http_packets = count,
            "dns_packets" => summary.dns_packets = count,
            "retransmissions" => summary.retransmissions = count,
            "duplicate_acks" => summary.duplicate_acks = count,
            "resets" => summary.resets = count,
            _ => {}
        }
    }

    summary.transport_shares = compute_transport_shares(&summary);
    summary.top_endpoints = top_n_endpoints(
        endpoint_counts(tshark_path, &plan.pcap_path).unwrap_or_default(),
        5,
    );
    summary.top_ports = top_n_ports(
        port_counts(tshark_path, &plan.pcap_path).unwrap_or_default(),
        5,
    );
    apply_interpretation(&mut summary);

    Ok(summary)
}

fn likely_target_endpoints(rows: &[EndpointPacketCount], targets: &[String]) -> Vec<String> {
    if targets.is_empty() {
        return rows.iter().take(2).map(|r| r.endpoint.clone()).collect();
    }
    let mut hints = vec![];
    for target in targets {
        for candidate in endpoint_candidates_from_target(target) {
            if rows.iter().any(|r| r.endpoint == candidate) && !hints.contains(&candidate) {
                hints.push(candidate);
            }
        }
    }
    if hints.is_empty() {
        rows.iter().take(2).map(|r| r.endpoint.clone()).collect()
    } else {
        hints
    }
}

fn endpoint_candidates_from_target(target: &str) -> Vec<String> {
    let mut out = vec![];
    let trimmed = target.trim();
    if let Ok(url) = url::Url::parse(trimmed) {
        if let Some(host) = url.host_str() {
            out.push(host.to_string());
        }
    }
    if !trimmed.contains("//") {
        let host = trimmed
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(':')
            .next()
            .unwrap_or(trimmed)
            .to_string();
        if !host.is_empty() {
            out.push(host);
        }
    }
    out.sort();
    out.dedup();
    out
}

fn endpoint_counts(tshark_path: &Path, pcap_path: &Path) -> anyhow::Result<BTreeMap<String, u64>> {
    let out = Command::new(tshark_path)
        .arg("-r")
        .arg(pcap_path)
        .arg("-T")
        .arg("fields")
        .arg("-e")
        .arg("ip.dst")
        .arg("-e")
        .arg("ipv6.dst")
        .output()
        .context("run tshark endpoint summary")?;

    if !out.status.success() {
        tracing::warn!(stderr = %String::from_utf8_lossy(&out.stderr), "tshark endpoint summary failed");
        return Ok(BTreeMap::new());
    }

    let mut counts = BTreeMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let endpoint = line
            .split('\t')
            .find(|v| !v.trim().is_empty())
            .unwrap_or("")
            .trim();
        if endpoint.is_empty() {
            continue;
        }
        *counts.entry(endpoint.to_string()).or_insert(0) += 1;
    }
    Ok(counts)
}

fn port_counts(tshark_path: &Path, pcap_path: &Path) -> anyhow::Result<BTreeMap<u16, u64>> {
    let out = Command::new(tshark_path)
        .arg("-r")
        .arg(pcap_path)
        .arg("-T")
        .arg("fields")
        .arg("-e")
        .arg("tcp.dstport")
        .arg("-e")
        .arg("udp.dstport")
        .output()
        .context("run tshark port summary")?;

    if !out.status.success() {
        tracing::warn!(stderr = %String::from_utf8_lossy(&out.stderr), "tshark port summary failed");
        return Ok(BTreeMap::new());
    }

    let mut counts = BTreeMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        for raw in line.split('\t') {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            if let Ok(port) = raw.parse::<u16>() {
                *counts.entry(port).or_insert(0) += 1;
                break;
            }
        }
    }
    Ok(counts)
}

fn compute_transport_shares(summary: &PacketCaptureSummary) -> Vec<PacketShare> {
    let total = summary.total_packets as f64;
    let mut rows = vec![
        PacketShare {
            protocol: "tcp".into(),
            packets: summary.tcp_packets,
            pct_of_total: pct(summary.tcp_packets, total),
        },
        PacketShare {
            protocol: "udp".into(),
            packets: summary.udp_packets,
            pct_of_total: pct(summary.udp_packets, total),
        },
        PacketShare {
            protocol: "quic".into(),
            packets: summary.quic_packets,
            pct_of_total: pct(summary.quic_packets, total),
        },
        PacketShare {
            protocol: "http".into(),
            packets: summary.http_packets,
            pct_of_total: pct(summary.http_packets, total),
        },
        PacketShare {
            protocol: "dns".into(),
            packets: summary.dns_packets,
            pct_of_total: pct(summary.dns_packets, total),
        },
    ];
    rows.sort_by(|a, b| {
        b.packets
            .cmp(&a.packets)
            .then_with(|| a.protocol.cmp(&b.protocol))
    });
    rows
}

fn pct(packets: u64, total: f64) -> f64 {
    if total <= 0.0 {
        0.0
    } else {
        ((packets as f64 / total) * 1000.0).round() / 10.0
    }
}

fn top_n_endpoints(counts: BTreeMap<String, u64>, limit: usize) -> Vec<EndpointPacketCount> {
    let mut rows: Vec<_> = counts
        .into_iter()
        .map(|(endpoint, packets)| EndpointPacketCount { endpoint, packets })
        .collect();
    rows.sort_by(|a, b| {
        b.packets
            .cmp(&a.packets)
            .then_with(|| a.endpoint.cmp(&b.endpoint))
    });
    rows.truncate(limit);
    rows
}

fn top_n_ports(counts: BTreeMap<u16, u64>, limit: usize) -> Vec<PortPacketCount> {
    let mut rows: Vec<_> = counts
        .into_iter()
        .map(|(port, packets)| PortPacketCount { port, packets })
        .collect();
    rows.sort_by(|a, b| b.packets.cmp(&a.packets).then_with(|| a.port.cmp(&b.port)));
    rows.truncate(limit);
    rows
}

fn apply_interpretation(summary: &mut PacketCaptureSummary) {
    summary.observed_quic = summary.quic_packets > 0;
    summary.observed_tcp_only =
        summary.tcp_packets > 0 && summary.udp_packets == 0 && summary.quic_packets == 0;
    summary.observed_mixed_transport =
        summary.tcp_packets > 0 && (summary.udp_packets > 0 || summary.quic_packets > 0);

    if summary.total_packets == 0 {
        summary.capture_may_be_ambiguous = true;
        summary
            .warnings
            .push("Capture completed but no packets were summarized from the trace.".into());
    }
    if summary.observed_tcp_only {
        let msg = "Capture succeeded, but no UDP/QUIC packets were observed in this trace. The workload may have stayed on TCP/TLS or loopback visibility may differ by stack/path.".to_string();
        summary.note = Some(msg.clone());
        summary.warnings.push(msg);
    }
    if summary.observed_mixed_transport {
        summary.capture_may_be_ambiguous = true;
        summary.warnings.push("Both TCP and UDP/QUIC traffic were observed. This may reflect fallback behavior, mixed page assets, or unrelated background traffic.".into());
    }
    if summary.top_endpoints.len() > 1 && summary.likely_target_endpoints.len() != 1 {
        summary.capture_may_be_ambiguous = true;
        summary.warnings.push("Multiple destination endpoints were active in the trace. Interpret protocol comparisons carefully when third-party assets or background traffic are present.".into());
    }
    if summary.likely_target_endpoints.is_empty() && !summary.top_endpoints.is_empty() {
        summary.capture_may_be_ambiguous = true;
        summary.warnings.push("No clear target-related endpoint could be identified from the dominant trace endpoints.".into());
    }
    if summary.retransmissions > 0 || summary.resets > 0 {
        summary.warnings.push("The trace includes transport-level error signals (retransmissions or resets) that may materially affect timing comparisons.".into());
    }
}

fn count_matches(tshark_path: &Path, pcap_path: &Path, filter: &str) -> anyhow::Result<u64> {
    let out = Command::new(tshark_path)
        .arg("-r")
        .arg(pcap_path)
        .arg("-Y")
        .arg(filter)
        .arg("-T")
        .arg("fields")
        .arg("-e")
        .arg("frame.number")
        .output()
        .with_context(|| format!("run tshark filter {filter}"))?;

    if !out.status.success() {
        tracing::warn!(
            filter,
            stderr = %String::from_utf8_lossy(&out.stderr),
            "tshark filter command failed while summarizing capture"
        );
        return Ok(0);
    }

    let count = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count() as u64;
    Ok(count)
}

fn which(name: &str) -> Option<PathBuf> {
    let candidate = PathBuf::from(name);
    if candidate.is_absolute() && candidate.exists() {
        return Some(candidate);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join(name))
            .find(|p| p.exists())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ResolvedPacketCaptureConfig;

    fn sample_cfg(mode: PacketCaptureMode, targets: Vec<&str>) -> ResolvedConfig {
        ResolvedConfig {
            targets: targets.into_iter().map(str::to_string).collect(),
            modes: vec![],
            runs: 1,
            concurrency: 1,
            timeout: 1000,
            payload_size: 0,
            payload_sizes: vec![],
            udp_port: 9999,
            udp_throughput_port: 9998,
            udp_probes: 20,
            connection_reuse: false,
            dns_enabled: true,
            ipv4_only: false,
            ipv6_only: false,
            no_proxy: false,
            proxy: None,
            ca_bundle: None,
            insecure: true,
            retries: 0,
            output_dir: "./out".into(),
            html_report: "report.html".into(),
            css: None,
            excel: false,
            save_to_db: false,
            db_url: None,
            db_migrate: false,
            save_to_sql: false,
            connection_string: None,
            log_level: None,
            page_asset_sizes: vec![],
            page_preset_name: None,
            http_stacks: vec![],
            packet_capture: ResolvedPacketCaptureConfig {
                mode,
                install_requirements: false,
                interface: "auto".into(),
                write_pcap: true,
                write_summary_json: true,
            },
            impairment: crate::cli::ResolvedImpairmentConfig {
                profile: crate::cli::ImpairmentProfile::None,
                delay_ms: 0,
            },
            json_stdout: false,
        }
    }

    #[test]
    fn mode_scope_helpers_are_correct() {
        assert!(PacketCaptureMode::Tester.captures_tester());
        assert!(!PacketCaptureMode::Tester.captures_endpoint());
        assert!(PacketCaptureMode::Endpoint.captures_endpoint());
        assert!(!PacketCaptureMode::Endpoint.captures_tester());
        assert!(PacketCaptureMode::Both.captures_tester());
        assert!(PacketCaptureMode::Both.captures_endpoint());
        assert!(!PacketCaptureMode::None.captures_tester());
    }

    #[test]
    fn build_plan_skips_when_tester_capture_disabled() {
        let cfg = sample_cfg(
            PacketCaptureMode::Endpoint,
            vec!["https://127.0.0.1:8443/health"],
        );
        assert!(build_plan(&cfg, Path::new("/tmp")).is_none());
    }

    #[test]
    fn build_plan_uses_loopback_interface_for_local_targets() {
        let cfg = sample_cfg(
            PacketCaptureMode::Tester,
            vec!["https://127.0.0.1:8443/health"],
        );
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let plan = build_plan(&cfg, Path::new("/tmp")).expect("plan");
            #[cfg(target_os = "macos")]
            assert_eq!(plan.interface, "lo0");
            #[cfg(target_os = "linux")]
            assert_eq!(plan.interface, "lo");
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = cfg;
        }
    }

    #[test]
    fn resolve_capture_interface_keeps_explicit_value() {
        assert_eq!(
            resolve_capture_interface("en7", &["https://example.com".into()]),
            "en7"
        );
    }

    #[test]
    fn resolve_capture_interface_remote_macos_defaults_to_en0() {
        #[cfg(target_os = "macos")]
        assert_eq!(
            resolve_capture_interface("auto", &["https://example.com".into()]),
            "en0"
        );
    }

    #[test]
    fn which_returns_absolute_existing_path() {
        let me = PathBuf::from("/bin/sh");
        if me.exists() {
            assert_eq!(which(me.to_str().unwrap()), Some(me));
        }
    }

    #[test]
    fn compute_transport_shares_sorts_by_packets() {
        let summary = PacketCaptureSummary {
            mode: "tester".into(),
            interface: "lo0".into(),
            capture_path: "x".into(),
            tshark_path: "tshark".into(),
            total_packets: 100,
            capture_status: "captured".into(),
            note: None,
            warnings: vec![],
            likely_target_endpoints: vec![],
            tcp_packets: 60,
            udp_packets: 40,
            quic_packets: 30,
            http_packets: 10,
            dns_packets: 5,
            retransmissions: 0,
            duplicate_acks: 0,
            resets: 0,
            transport_shares: vec![],
            top_endpoints: vec![],
            top_ports: vec![],
            observed_quic: false,
            observed_tcp_only: false,
            observed_mixed_transport: false,
            capture_may_be_ambiguous: false,
        };
        let shares = compute_transport_shares(&summary);
        assert_eq!(shares[0].protocol, "tcp");
        assert_eq!(shares[0].pct_of_total, 60.0);
        assert_eq!(shares[1].protocol, "udp");
    }

    #[test]
    fn likely_target_endpoints_prefers_matching_target_host() {
        let rows = vec![
            EndpointPacketCount {
                endpoint: "198.51.100.10".into(),
                packets: 50,
            },
            EndpointPacketCount {
                endpoint: "203.0.113.5".into(),
                packets: 10,
            },
        ];
        let hints = likely_target_endpoints(&rows, &["https://198.51.100.10:8443/health".into()]);
        assert_eq!(hints, vec!["198.51.100.10".to_string()]);
    }

    #[test]
    fn apply_interpretation_marks_mixed_transport_as_ambiguous() {
        let mut summary = PacketCaptureSummary {
            mode: "tester".into(),
            interface: "lo0".into(),
            capture_path: "x".into(),
            tshark_path: "tshark".into(),
            total_packets: 50,
            capture_status: "captured".into(),
            note: None,
            warnings: vec![],
            likely_target_endpoints: vec![],
            tcp_packets: 20,
            udp_packets: 15,
            quic_packets: 10,
            http_packets: 0,
            dns_packets: 0,
            retransmissions: 0,
            duplicate_acks: 0,
            resets: 0,
            transport_shares: vec![],
            top_endpoints: vec![
                EndpointPacketCount {
                    endpoint: "1.1.1.1".into(),
                    packets: 20,
                },
                EndpointPacketCount {
                    endpoint: "2.2.2.2".into(),
                    packets: 10,
                },
            ],
            top_ports: vec![],
            observed_quic: false,
            observed_tcp_only: false,
            observed_mixed_transport: false,
            capture_may_be_ambiguous: false,
        };
        apply_interpretation(&mut summary);
        assert!(summary.observed_quic);
        assert!(summary.observed_mixed_transport);
        assert!(summary.capture_may_be_ambiguous);
        assert!(!summary.warnings.is_empty());
    }
}
