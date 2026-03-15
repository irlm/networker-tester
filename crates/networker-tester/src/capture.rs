use anyhow::Context;
use serde::Serialize;
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

#[derive(Debug, Serialize)]
pub struct PacketCaptureSummary {
    pub mode: String,
    pub interface: String,
    pub capture_path: String,
    pub tshark_path: String,
    pub total_packets: u64,
    pub capture_status: String,
    pub note: Option<String>,
    pub tcp_packets: u64,
    pub udp_packets: u64,
    pub quic_packets: u64,
    pub http_packets: u64,
    pub dns_packets: u64,
    pub retransmissions: u64,
    pub duplicate_acks: u64,
    pub resets: u64,
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
        let summary = spawn_blocking(move || summarize(&tshark_path, &plan))
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

fn summarize(tshark_path: &Path, plan: &PacketCapturePlan) -> anyhow::Result<PacketCaptureSummary> {
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
        tcp_packets: 0,
        udp_packets: 0,
        quic_packets: 0,
        http_packets: 0,
        dns_packets: 0,
        retransmissions: 0,
        duplicate_acks: 0,
        resets: 0,
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

    if summary.total_packets > 0 && summary.udp_packets == 0 && summary.quic_packets == 0 {
        summary.note = Some(
            "Capture succeeded, but no UDP/QUIC packets were observed in this trace. The workload may have stayed on TCP/TLS or loopback visibility may differ by stack/path.".into(),
        );
    }

    Ok(summary)
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
}
