use crate::metrics::{TestRun, UrlTestRun};
use crate::tls_profile::TlsEndpointProfile;
use std::path::Path;

/// Serialize a `TestRun` to pretty-printed JSON and write to `path`.
pub fn save(run: &TestRun, path: &Path) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let json = to_string(run)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Return the JSON string without writing to disk (useful for testing).
pub fn to_string(run: &TestRun) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(run)?)
}

/// Serialize a `UrlTestRun` to pretty-printed JSON and write to `path`.
pub fn save_url_test(run: &UrlTestRun, path: &Path) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(run)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Return the JSON string for a `UrlTestRun` without writing to disk.
pub fn to_string_url_test(run: &UrlTestRun) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(run)?)
}

/// Serialize a `TlsEndpointProfile` to pretty-printed JSON and write to `path`.
pub fn save_tls_profile(run: &TlsEndpointProfile, path: &Path) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let json = to_string_tls_profile(run)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Return the JSON string for a `TlsEndpointProfile` without writing to disk.
pub fn to_string_tls_profile(run: &TlsEndpointProfile) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(run)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{
        Protocol, RequestAttempt, TestRun, UrlConnectionSummary, UrlDiagnosticStatus,
        UrlOriginSummary, UrlPageLoadStrategy, UrlTestRun,
    };
    use crate::tls_profile::{
        TlsCapabilitiesSection, TlsCertificateSection, TlsChainDiagnostics, TlsClientAuthStatus,
        TlsEndpointProfile, TlsPathCharacteristics, TlsPathClassification, TlsProfileConnectivity,
        TlsProfileCoverageLevel, TlsProfileSummary, TlsProfileTarget, TlsProfileTargetKind,
        TlsProtocolSupport, TlsResumptionSection, TlsRevocationInfo, TlsSniBehavior,
        TlsTrustSection,
    };
    use chrono::Utc;
    use tempfile::NamedTempFile;
    use uuid::Uuid;
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
            packet_capture_summary: None,
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
    fn save_creates_file() {
        let tmp = NamedTempFile::new().unwrap();
        let run = dummy_run();
        save(&run, tmp.path()).unwrap();
        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(contents.contains("\"target_url\""));
    }

    fn dummy_url_test_run() -> UrlTestRun {
        UrlTestRun {
            id: Uuid::new_v4(),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            requested_url: "https://example.com".into(),
            final_url: Some("https://www.example.com".into()),
            status: UrlDiagnosticStatus::Completed,
            page_load_strategy: UrlPageLoadStrategy::Browser,
            browser_engine: Some("chromium".into()),
            browser_version: Some("123.0".into()),
            user_agent: Some("NetworkerTester/0.13".into()),
            primary_origin: Some("https://www.example.com".into()),
            observed_protocol_primary_load: Some("h3".into()),
            advertised_alt_svc: None,
            validated_http_versions: vec!["h3".into()],
            tls_version: None,
            cipher_suite: None,
            alpn: Some("h3".into()),
            dns_ms: Some(10.0),
            connect_ms: Some(20.0),
            handshake_ms: Some(25.0),
            ttfb_ms: Some(50.0),
            dom_content_loaded_ms: Some(150.0),
            load_event_ms: Some(300.0),
            network_idle_ms: None,
            capture_end_ms: Some(300.0),
            total_requests: 4,
            total_transfer_bytes: 4096,
            peak_concurrent_connections: None,
            redirect_count: 1,
            failure_count: 0,
            har_path: None,
            pcap_path: None,
            pcap_summary: None,
            capture_errors: vec![],
            environment_notes: None,
            origin_summaries: vec![UrlOriginSummary {
                origin: "https://www.example.com".into(),
                request_count: 4,
                failure_count: 0,
                total_transfer_bytes: 4096,
                protocols: vec!["h3".into()],
                dominant_protocol: Some("h3".into()),
                average_duration_ms: Some(12.5),
                cache_hit_count: Some(1),
            }],
            connection_summary: Some(UrlConnectionSummary {
                total_connection_ids: 1,
                reused_connection_count: 1,
                reused_resource_count: 3,
                resources_with_connection_id: 4,
                peak_origin_request_count: Some(4),
            }),
            resources: vec![],
            protocol_runs: vec![],
        }
    }

    #[test]
    fn url_test_json_round_trip() {
        let run = dummy_url_test_run();
        let json = to_string_url_test(&run).unwrap();
        let de: UrlTestRun = serde_json::from_str(&json).unwrap();
        assert_eq!(de.id, run.id);
        assert_eq!(de.requested_url, run.requested_url);
    }

    #[test]
    fn save_url_test_creates_file() {
        let tmp = NamedTempFile::new().unwrap();
        let run = dummy_url_test_run();
        save_url_test(&run, tmp.path()).unwrap();
        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(contents.contains("\"requested_url\""));
    }

    #[test]
    fn trust_section_deserializes_without_new_fields() {
        let old_json = r#"{
            "hostname_matches": true,
            "chain_valid": true,
            "trusted_by_system_store": true,
            "issues": [],
            "revocation": {
                "ocsp_stapled": false,
                "method": "best_effort",
                "status": "unknown",
                "notes": []
            }
        }"#;
        let section: TlsTrustSection = serde_json::from_str(old_json).unwrap();
        assert!(!section.verification_performed);
        assert!(!section.chain_presented);
        assert_eq!(section.chain_diagnostics.presented_chain_length, 0);
        assert!(section.revocation.ocsp_urls.is_empty());
        assert!(!section.revocation.online_check_attempted);
    }

    #[test]
    fn tls_profile_json_round_trip() {
        let run = TlsEndpointProfile {
            target_kind: TlsProfileTargetKind::ExternalUrl,
            coverage_level: TlsProfileCoverageLevel::ClientObserved,
            unsupported_checks: vec!["protocol_matrix".into()],
            limitations: vec!["client-visible only".into()],
            target: TlsProfileTarget {
                host: "example.com".into(),
                port: 443,
                requested_ip: None,
                sni: Some("example.com".into()),
                resolved_ips: vec!["93.184.216.34".into()],
                source_url: Some("https://example.com".into()),
            },
            path_characteristics: TlsPathCharacteristics {
                connected_ip: Some("93.184.216.34".into()),
                direct_ip_match: true,
                proxy_detected: false,
                classification: TlsPathClassification::Direct,
                evidence: vec![],
            },
            connectivity: TlsProfileConnectivity {
                tcp_connect_ms: Some(10.0),
                tls_handshake_ms: Some(20.0),
                negotiated_tls_version: Some("TLSv1_3".into()),
                negotiated_cipher_suite: Some("TLS_AES_128_GCM_SHA256".into()),
                negotiated_key_exchange_group: None,
                alpn: Some("h2".into()),
            },
            certificate: TlsCertificateSection {
                leaf: None,
                chain: vec![],
            },
            trust: TlsTrustSection {
                hostname_matches: true,
                chain_valid: true,
                trusted_by_system_store: true,
                verification_performed: true,
                chain_presented: false,
                verified_chain_depth: None,
                issues: vec![],
                chain_diagnostics: TlsChainDiagnostics {
                    presented_chain_length: 0,
                    leaf_self_signed: false,
                    has_intermediate: false,
                    ordered_subject_issuer_links: true,
                    root_included: false,
                    notes: vec![],
                },
                revocation: TlsRevocationInfo {
                    ocsp_stapled: false,
                    method: "best_effort".into(),
                    status: "unknown".into(),
                    ocsp_urls: vec![],
                    crl_urls: vec![],
                    online_check_attempted: false,
                    notes: vec![],
                },
                caa: None,
            },
            capabilities: Some(TlsCapabilitiesSection {
                protocol_support: vec![TlsProtocolSupport {
                    protocol: "tls1.3".into(),
                    supported: true,
                    accepted_ciphers: vec!["TLS_AES_128_GCM_SHA256".into()],
                    supported_groups: vec!["X25519".into()],
                }],
                alpn_support: vec!["h2".into()],
                sni_behavior: TlsSniBehavior {
                    with_sni_ok: true,
                    without_sni_ok: Some(false),
                    default_cert_subject: None,
                },
                client_auth: TlsClientAuthStatus {
                    requested: false,
                    required: false,
                },
            }),
            resumption: TlsResumptionSection {
                supported: false,
                method: None,
                initial_handshake_ms: Some(20.0),
                resumed_handshake_ms: Some(18.0),
                early_data_offered: false,
                early_data_accepted: None,
            },
            findings: vec![],
            summary: TlsProfileSummary {
                status: "ok".into(),
                score: None,
            },
        };
        let json = to_string_tls_profile(&run).unwrap();
        let de: TlsEndpointProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(de.target.host, "example.com");
        assert!(de.summary.score.is_none());
    }
}
