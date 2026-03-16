use crate::{capture::PacketCaptureSummary, metrics::TestRun};
use std::path::Path;

/// Serialize a `TestRun` to pretty-printed JSON and write to `path`.
pub fn save(
    run: &TestRun,
    path: &Path,
    packet_capture: Option<&PacketCaptureSummary>,
) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let json = to_string_with_capture(run, packet_capture)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Return the JSON string without writing to disk (useful for testing).
pub fn to_string(run: &TestRun) -> anyhow::Result<String> {
    to_string_with_capture(run, None)
}

pub fn to_string_with_capture(
    run: &TestRun,
    packet_capture: Option<&PacketCaptureSummary>,
) -> anyhow::Result<String> {
    let value = match packet_capture {
        Some(summary) => serde_json::json!({
            "run": run,
            "packet_capture_summary": summary,
        }),
        None => serde_json::to_value(run)?,
    };
    Ok(serde_json::to_string_pretty(&value)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        capture::{EndpointPacketCount, PacketCaptureSummary, PacketShare, PortPacketCount},
        metrics::{Protocol, RequestAttempt, TestRun},
    };
    use chrono::Utc;
    use tempfile::NamedTempFile;
    use uuid::Uuid;

    fn sample_packet_capture_summary() -> PacketCaptureSummary {
        PacketCaptureSummary {
            mode: "tester".into(),
            interface: "lo0".into(),
            capture_path: "packet-capture-tester.pcapng".into(),
            tshark_path: "tshark".into(),
            total_packets: 42,
            capture_status: "captured".into(),
            note: Some("mixed trace".into()),
            warnings: vec!["warning one".into()],
            likely_target_endpoints: vec!["127.0.0.1".into()],
            likely_target_packets: 20,
            likely_target_pct_of_total: 47.6,
            dominant_target_port: Some(443),
            capture_confidence: "medium".into(),
            tcp_packets: 10,
            udp_packets: 20,
            quic_packets: 15,
            http_packets: 5,
            dns_packets: 2,
            retransmissions: 1,
            duplicate_acks: 0,
            resets: 0,
            transport_shares: vec![PacketShare {
                protocol: "udp".into(),
                packets: 20,
                pct_of_total: 47.6,
            }],
            top_endpoints: vec![EndpointPacketCount {
                endpoint: "127.0.0.1".into(),
                packets: 20,
            }],
            top_ports: vec![PortPacketCount {
                port: 443,
                packets: 18,
            }],
            observed_quic: true,
            observed_tcp_only: false,
            observed_mixed_transport: true,
            capture_may_be_ambiguous: true,
        }
    }

    fn dummy_run() -> TestRun {
        let run_id = Uuid::new_v4();
        TestRun {
            run_id,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            target_url: "http://localhost/health".into(),
            target_host: "localhost".into(),
            modes: vec!["http1".into()],
            total_runs: 1,
            concurrency: 1,
            timeout_ms: 5000,
            client_os: "test".into(),
            client_version: "0.1.0".into(),
            server_info: None,
            client_info: None,
            baseline: None,
            attempts: vec![RequestAttempt {
                attempt_id: Uuid::new_v4(),
                run_id,
                protocol: Protocol::Http1,
                sequence_num: 0,
                started_at: Utc::now(),
                finished_at: Some(Utc::now()),
                success: true,
                dns: None,
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
            }],
        }
    }

    #[test]
    fn json_round_trip() {
        let run = dummy_run();
        let json = to_string(&run).unwrap();
        let de: TestRun = serde_json::from_str(&json).unwrap();
        assert_eq!(de.run_id, run.run_id);
        assert_eq!(de.attempts.len(), 1);
    }

    #[test]
    fn to_string_with_capture_includes_packet_capture_summary() {
        let run = dummy_run();
        let json = to_string_with_capture(&run, Some(&sample_packet_capture_summary())).unwrap();
        assert!(json.contains("\"packet_capture_summary\""));
        assert!(json.contains("\"likely_target_endpoints\""));
        assert!(json.contains("\"observed_quic\": true"));
        assert!(json.contains("\"capture_confidence\": \"medium\""));
    }

    #[test]
    fn save_creates_file() {
        let tmp = NamedTempFile::new().unwrap();
        let run = dummy_run();
        save(&run, tmp.path(), None).unwrap();
        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(contents.contains("\"target_url\""));
    }
}
