use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct UrlTestSummary {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub requested_url: String,
    pub final_url: Option<String>,
    pub status: String,
    pub page_load_strategy: String,
    pub observed_protocol_primary_load: Option<String>,
    pub total_requests: i32,
    pub total_transfer_bytes: i64,
    pub failure_count: i32,
}

#[derive(Debug, Serialize)]
pub struct UrlTestResourceRow {
    pub resource_url: String,
    pub origin: String,
    pub resource_type: String,
    pub mime_type: Option<String>,
    pub status_code: Option<i32>,
    pub protocol: Option<String>,
    pub transfer_size: Option<i64>,
    pub encoded_body_size: Option<i64>,
    pub decoded_body_size: Option<i64>,
    pub duration_ms: Option<f64>,
    pub connection_id: Option<String>,
    pub reused_connection: Option<bool>,
    pub initiator_type: Option<String>,
    pub from_cache: Option<bool>,
    pub redirected: Option<bool>,
    pub failed: bool,
}

#[derive(Debug, Serialize)]
pub struct UrlTestProtocolRunRow {
    pub protocol_mode: String,
    pub run_number: i32,
    pub attempt_type: String,
    pub observed_protocol: Option<String>,
    pub fallback_occurred: Option<bool>,
    pub succeeded: bool,
    pub status_code: Option<i32>,
    pub ttfb_ms: Option<f64>,
    pub total_ms: Option<f64>,
    pub failure_reason: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UrlTestOverview {
    pub status: String,
    pub requested_url: String,
    pub final_url: Option<String>,
    pub primary_origin: Option<String>,
    pub observed_protocol_primary_load: Option<String>,
    pub browser_engine: Option<String>,
    pub browser_version: Option<String>,
    pub total_requests: i32,
    pub total_transfer_bytes: i64,
    pub failure_count: i32,
    pub redirect_count: i32,
}

#[derive(Debug, Serialize)]
pub struct UrlTestTimingSummary {
    pub dns_ms: Option<f64>,
    pub connect_ms: Option<f64>,
    pub handshake_ms: Option<f64>,
    pub ttfb_ms: Option<f64>,
    pub dom_content_loaded_ms: Option<f64>,
    pub load_event_ms: Option<f64>,
    pub network_idle_ms: Option<f64>,
    pub capture_end_ms: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct UrlTestProtocolSummary {
    pub page_load_strategy: String,
    pub observed_protocol_primary_load: Option<String>,
    pub validated_http_versions: Vec<String>,
    pub advertised_alt_svc: Option<String>,
    pub alpn: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UrlTestTlsSummary {
    pub tls_version: Option<String>,
    pub cipher_suite: Option<String>,
    pub alpn: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlPacketCaptureSummaryView {
    pub mode: String,
    pub interface: String,
    pub capture_path: String,
    pub total_packets: u64,
    pub capture_status: String,
    pub note: Option<String>,
    pub warnings: Vec<String>,
    pub tcp_packets: u64,
    pub udp_packets: u64,
    pub quic_packets: u64,
    pub http_packets: u64,
    pub dns_packets: u64,
    pub retransmissions: u64,
    pub duplicate_acks: u64,
    pub resets: u64,
    pub observed_quic: bool,
    pub observed_tcp_only: bool,
    pub observed_mixed_transport: bool,
    pub capture_may_be_ambiguous: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlOriginSummaryView {
    pub origin: String,
    pub request_count: u32,
    pub failure_count: u32,
    pub total_transfer_bytes: u64,
    pub protocols: Vec<String>,
    pub dominant_protocol: Option<String>,
    pub average_duration_ms: Option<f64>,
    pub cache_hit_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlConnectionSummaryView {
    pub total_connection_ids: u32,
    pub reused_connection_count: u32,
    pub reused_resource_count: u32,
    pub resources_with_connection_id: u32,
    pub peak_origin_request_count: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct UrlTestArtifactSummary {
    pub har_path: Option<String>,
    pub pcap_path: Option<String>,
    pub pcap_summary: Option<UrlPacketCaptureSummaryView>,
    pub capture_errors: Vec<String>,
    pub environment_notes: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UrlTestSectionedDetail {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub overview: UrlTestOverview,
    pub timings: UrlTestTimingSummary,
    pub protocol: UrlTestProtocolSummary,
    pub tls: UrlTestTlsSummary,
    pub artifacts: UrlTestArtifactSummary,
    pub origin_summaries: Vec<UrlOriginSummaryView>,
    pub connection_summary: Option<UrlConnectionSummaryView>,
    pub resources: Vec<UrlTestResourceRow>,
    pub protocol_runs: Vec<UrlTestProtocolRunRow>,
}

#[derive(Debug, Serialize)]
pub struct UrlTestDetail {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub requested_url: String,
    pub final_url: Option<String>,
    pub status: String,
    pub page_load_strategy: String,
    pub browser_engine: Option<String>,
    pub browser_version: Option<String>,
    pub user_agent: Option<String>,
    pub primary_origin: Option<String>,
    pub observed_protocol_primary_load: Option<String>,
    pub advertised_alt_svc: Option<String>,
    pub validated_http_versions: Vec<String>,
    pub tls_version: Option<String>,
    pub cipher_suite: Option<String>,
    pub alpn: Option<String>,
    pub dns_ms: Option<f64>,
    pub connect_ms: Option<f64>,
    pub handshake_ms: Option<f64>,
    pub ttfb_ms: Option<f64>,
    pub dom_content_loaded_ms: Option<f64>,
    pub load_event_ms: Option<f64>,
    pub network_idle_ms: Option<f64>,
    pub capture_end_ms: Option<f64>,
    pub total_requests: i32,
    pub total_transfer_bytes: i64,
    pub peak_concurrent_connections: Option<i32>,
    pub redirect_count: i32,
    pub failure_count: i32,
    pub har_path: Option<String>,
    pub pcap_path: Option<String>,
    pub pcap_summary: Option<UrlPacketCaptureSummaryView>,
    pub capture_errors: Vec<String>,
    pub environment_notes: Option<String>,
    pub origin_summaries: Vec<UrlOriginSummaryView>,
    pub connection_summary: Option<UrlConnectionSummaryView>,
    pub resources: Vec<UrlTestResourceRow>,
    pub protocol_runs: Vec<UrlTestProtocolRunRow>,
}

pub async fn list(
    client: &Client,
    project_id: &Uuid,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<UrlTestSummary>> {
    // UrlTestRun links to TestRun (via Id = RunId relationship from jobs).
    // Filter through job table: job.run_id matches a TestRun.RunId,
    // and UrlTestRun.Id links to TestRun.RunId through the same run concept.
    // For backward compat, also include url tests not linked to any job.
    let rows = client
        .query(
            "SELECT u.Id, u.StartedAt, u.CompletedAt, u.RequestedUrl, u.FinalUrl, u.Status,
                    u.PageLoadStrategy, u.ObservedProtocolPrimaryLoad, u.TotalRequests,
                    u.TotalTransferBytes, u.FailureCount
             FROM UrlTestRun u
             WHERE EXISTS (
                 SELECT 1 FROM job j WHERE j.project_id = $1
                 AND j.run_id IN (SELECT RunId FROM TestRun WHERE RunId = u.Id)
             )
             OR NOT EXISTS (
                 SELECT 1 FROM job j2 WHERE j2.run_id IN (SELECT RunId FROM TestRun WHERE RunId = u.Id)
             )
             ORDER BY u.StartedAt DESC LIMIT $2 OFFSET $3",
            &[project_id, &limit, &offset],
        )
        .await?;

    Ok(rows
        .iter()
        .map(|r| UrlTestSummary {
            id: r.get("id"),
            started_at: r.get("startedat"),
            completed_at: r.get("completedat"),
            requested_url: r.get("requestedurl"),
            final_url: r.get("finalurl"),
            status: r.get("status"),
            page_load_strategy: r.get("pageloadstrategy"),
            observed_protocol_primary_load: r.get("observedprotocolprimaryload"),
            total_requests: r.get("totalrequests"),
            total_transfer_bytes: r.get("totaltransferbytes"),
            failure_count: r.get("failurecount"),
        })
        .collect())
}

fn split_csv_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn split_multiline_list(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn redact_path(path: Option<String>) -> Option<String> {
    path.and_then(|p| {
        Path::new(&p)
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToString::to_string)
            .or(Some(p))
    })
}

fn redact_pcap_summary_capture_path(
    pcap_summary: Option<UrlPacketCaptureSummaryView>,
) -> Option<UrlPacketCaptureSummaryView> {
    pcap_summary.map(|mut summary| {
        summary.capture_path = redact_path(Some(summary.capture_path)).unwrap_or_default();
        summary
    })
}

fn summarize_origins_and_connections(
    resources: &[UrlTestResourceRow],
) -> (Vec<UrlOriginSummaryView>, Option<UrlConnectionSummaryView>) {
    use std::collections::{BTreeMap, BTreeSet};

    #[derive(Default)]
    struct OriginAgg {
        request_count: u32,
        failure_count: u32,
        total_transfer_bytes: u64,
        protocol_counts: BTreeMap<String, u32>,
        duration_sum: f64,
        duration_count: u32,
        cache_hit_count: u32,
        cache_known_count: u32,
    }

    let mut by_origin: BTreeMap<String, OriginAgg> = BTreeMap::new();
    let mut connection_ids: BTreeSet<String> = BTreeSet::new();
    let mut reused_connection_count = 0u32;
    let mut reused_resource_count = 0u32;
    let mut resources_with_connection_id = 0u32;

    for resource in resources {
        let agg = by_origin.entry(resource.origin.clone()).or_default();
        agg.request_count += 1;
        if resource.failed {
            agg.failure_count += 1;
        }
        agg.total_transfer_bytes += resource.transfer_size.unwrap_or(0).max(0) as u64;
        if let Some(proto) = &resource.protocol {
            *agg.protocol_counts.entry(proto.clone()).or_insert(0) += 1;
        }
        if let Some(duration) = resource.duration_ms {
            agg.duration_sum += duration;
            agg.duration_count += 1;
        }
        if let Some(from_cache) = resource.from_cache {
            agg.cache_known_count += 1;
            if from_cache {
                agg.cache_hit_count += 1;
            }
        }
        if let Some(id) = &resource.connection_id {
            resources_with_connection_id += 1;
            connection_ids.insert(id.clone());
        }
        if resource.reused_connection == Some(true) {
            reused_resource_count += 1;
            if resource.connection_id.is_some() {
                reused_connection_count += 1;
            }
        }
    }

    let peak_origin_request_count = by_origin.values().map(|a| a.request_count).max();
    let origin_summaries = by_origin
        .into_iter()
        .map(|(origin, agg)| {
            let mut protocol_pairs = agg.protocol_counts.into_iter().collect::<Vec<_>>();
            protocol_pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let protocols = protocol_pairs
                .iter()
                .map(|(p, _)| p.clone())
                .collect::<Vec<_>>();
            let dominant_protocol = protocol_pairs.first().map(|(p, _)| p.clone());
            UrlOriginSummaryView {
                origin,
                request_count: agg.request_count,
                failure_count: agg.failure_count,
                total_transfer_bytes: agg.total_transfer_bytes,
                protocols,
                dominant_protocol,
                average_duration_ms: if agg.duration_count > 0 {
                    Some(agg.duration_sum / agg.duration_count as f64)
                } else {
                    None
                },
                cache_hit_count: if agg.cache_known_count > 0 {
                    Some(agg.cache_hit_count)
                } else {
                    None
                },
            }
        })
        .collect::<Vec<_>>();

    let connection_summary = if resources.is_empty() {
        None
    } else {
        Some(UrlConnectionSummaryView {
            total_connection_ids: connection_ids.len() as u32,
            reused_connection_count,
            reused_resource_count,
            resources_with_connection_id,
            peak_origin_request_count,
        })
    };

    (origin_summaries, connection_summary)
}

pub fn section_detail(detail: UrlTestDetail) -> UrlTestSectionedDetail {
    let (origin_summaries, connection_summary) =
        summarize_origins_and_connections(&detail.resources);
    UrlTestSectionedDetail {
        id: detail.id,
        started_at: detail.started_at,
        completed_at: detail.completed_at,
        overview: UrlTestOverview {
            status: detail.status.clone(),
            requested_url: detail.requested_url.clone(),
            final_url: detail.final_url.clone(),
            primary_origin: detail.primary_origin.clone(),
            observed_protocol_primary_load: detail.observed_protocol_primary_load.clone(),
            browser_engine: detail.browser_engine.clone(),
            browser_version: detail.browser_version.clone(),
            total_requests: detail.total_requests,
            total_transfer_bytes: detail.total_transfer_bytes,
            failure_count: detail.failure_count,
            redirect_count: detail.redirect_count,
        },
        timings: UrlTestTimingSummary {
            dns_ms: detail.dns_ms,
            connect_ms: detail.connect_ms,
            handshake_ms: detail.handshake_ms,
            ttfb_ms: detail.ttfb_ms,
            dom_content_loaded_ms: detail.dom_content_loaded_ms,
            load_event_ms: detail.load_event_ms,
            network_idle_ms: detail.network_idle_ms,
            capture_end_ms: detail.capture_end_ms,
        },
        protocol: UrlTestProtocolSummary {
            page_load_strategy: detail.page_load_strategy.clone(),
            observed_protocol_primary_load: detail.observed_protocol_primary_load.clone(),
            validated_http_versions: detail.validated_http_versions.clone(),
            advertised_alt_svc: detail.advertised_alt_svc.clone(),
            alpn: detail.alpn.clone(),
        },
        tls: UrlTestTlsSummary {
            tls_version: detail.tls_version.clone(),
            cipher_suite: detail.cipher_suite.clone(),
            alpn: detail.alpn.clone(),
        },
        artifacts: UrlTestArtifactSummary {
            har_path: redact_path(detail.har_path.clone()),
            pcap_path: redact_path(detail.pcap_path.clone()),
            pcap_summary: redact_pcap_summary_capture_path(detail.pcap_summary.clone()),
            capture_errors: detail.capture_errors.clone(),
            environment_notes: detail.environment_notes.clone(),
        },
        origin_summaries,
        connection_summary,
        resources: detail.resources,
        protocol_runs: detail.protocol_runs,
    }
}

pub async fn get(client: &Client, id: &Uuid) -> anyhow::Result<Option<UrlTestDetail>> {
    let row = client
        .query_opt(
            "SELECT Id, StartedAt, CompletedAt, RequestedUrl, FinalUrl, Status, PageLoadStrategy,
                    BrowserEngine, BrowserVersion, UserAgent, PrimaryOrigin, ObservedProtocolPrimaryLoad,
                    AdvertisedAltSvc, ValidatedHttpVersions, TlsVersion, CipherSuite, Alpn,
                    DnsMs, ConnectMs, HandshakeMs, TtfbMs, DomContentLoadedMs, LoadEventMs,
                    NetworkIdleMs, CaptureEndMs, TotalRequests, TotalTransferBytes,
                    PeakConcurrentConnections, RedirectCount, FailureCount, HarPath, PcapPath,
                    PcapSummaryJson, CaptureErrors, EnvironmentNotes
             FROM UrlTestRun WHERE Id = $1",
            &[id],
        )
        .await?;

    let Some(r) = row else {
        return Ok(None);
    };

    let resources_rows = client
        .query(
            "SELECT ResourceUrl, Origin, ResourceType, MimeType, StatusCode, Protocol,
                    TransferSize, EncodedBodySize, DecodedBodySize, DurationMs, ConnectionId,
                    ReusedConnection, InitiatorType, FromCache, Redirected, Failed
             FROM UrlTestResource WHERE UrlTestRunId = $1
             ORDER BY DurationMs DESC NULLS LAST, ResourceUrl ASC",
            &[id],
        )
        .await?;

    let protocol_rows = client
        .query(
            "SELECT ProtocolMode, RunNumber, AttemptType, ObservedProtocol, FallbackOccurred,
                    Succeeded, StatusCode, TtfbMs, TotalMs, FailureReason, Error
             FROM UrlTestProtocolRun WHERE UrlTestRunId = $1
             ORDER BY ProtocolMode ASC, RunNumber ASC",
            &[id],
        )
        .await?;

    let validated_http_versions = {
        let raw: String = r.get("validatedhttpversions");
        split_csv_list(&raw)
    };

    let capture_errors = {
        let raw: Option<String> = r.get("captureerrors");
        split_multiline_list(&raw.unwrap_or_default())
    };

    let pcap_summary = {
        let raw: Option<String> = r.get("pcapsummaryjson");
        raw.as_deref()
            .map(serde_json::from_str::<UrlPacketCaptureSummaryView>)
            .transpose()?
    };

    Ok(Some(UrlTestDetail {
        id: r.get("id"),
        started_at: r.get("startedat"),
        completed_at: r.get("completedat"),
        requested_url: r.get("requestedurl"),
        final_url: r.get("finalurl"),
        status: r.get("status"),
        page_load_strategy: r.get("pageloadstrategy"),
        browser_engine: r.get("browserengine"),
        browser_version: r.get("browserversion"),
        user_agent: r.get("useragent"),
        primary_origin: r.get("primaryorigin"),
        observed_protocol_primary_load: r.get("observedprotocolprimaryload"),
        advertised_alt_svc: r.get("advertisedaltsvc"),
        validated_http_versions,
        tls_version: r.get("tlsversion"),
        cipher_suite: r.get("ciphersuite"),
        alpn: r.get("alpn"),
        dns_ms: r.get("dnsms"),
        connect_ms: r.get("connectms"),
        handshake_ms: r.get("handshakems"),
        ttfb_ms: r.get("ttfbms"),
        dom_content_loaded_ms: r.get("domcontentloadedms"),
        load_event_ms: r.get("loadeventms"),
        network_idle_ms: r.get("networkidlems"),
        capture_end_ms: r.get("captureendms"),
        total_requests: r.get("totalrequests"),
        total_transfer_bytes: r.get("totaltransferbytes"),
        peak_concurrent_connections: r.get("peakconcurrentconnections"),
        redirect_count: r.get("redirectcount"),
        failure_count: r.get("failurecount"),
        har_path: redact_path(r.get("harpath")),
        pcap_path: redact_path(r.get("pcappath")),
        pcap_summary: redact_pcap_summary_capture_path(pcap_summary),
        capture_errors,
        environment_notes: r.get("environmentnotes"),
        origin_summaries: vec![],
        connection_summary: None,
        resources: resources_rows
            .iter()
            .map(|row| UrlTestResourceRow {
                resource_url: row.get("resourceurl"),
                origin: row.get("origin"),
                resource_type: row.get("resourcetype"),
                mime_type: row.get("mimetype"),
                status_code: row.get("statuscode"),
                protocol: row.get("protocol"),
                transfer_size: row.get("transfersize"),
                encoded_body_size: row.get("encodedbodysize"),
                decoded_body_size: row.get("decodedbodysize"),
                duration_ms: row.get("durationms"),
                connection_id: row.get("connectionid"),
                reused_connection: row.get("reusedconnection"),
                initiator_type: row.get("initiatortype"),
                from_cache: row.get("fromcache"),
                redirected: row.get("redirected"),
                failed: row.get("failed"),
            })
            .collect(),
        protocol_runs: protocol_rows
            .iter()
            .map(|row| UrlTestProtocolRunRow {
                protocol_mode: row.get("protocolmode"),
                run_number: row.get("runnumber"),
                attempt_type: row.get("attempttype"),
                observed_protocol: row.get("observedprotocol"),
                fallback_occurred: row.get("fallbackoccurred"),
                succeeded: row.get("succeeded"),
                status_code: row.get("statuscode"),
                ttfb_ms: row.get("ttfbms"),
                total_ms: row.get("totalms"),
                failure_reason: row.get("failurereason"),
                error: row.get("error"),
            })
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        section_detail, split_csv_list, split_multiline_list, UrlPacketCaptureSummaryView,
        UrlTestDetail, UrlTestProtocolRunRow, UrlTestResourceRow,
    };
    use chrono::Utc;
    use uuid::Uuid;

    #[test]
    fn split_csv_list_ignores_empty_entries() {
        assert_eq!(split_csv_list("h1, h2,,h3 "), vec!["h1", "h2", "h3"]);
    }

    #[test]
    fn split_multiline_list_ignores_blank_lines() {
        assert_eq!(
            split_multiline_list("first\n\n second \n"),
            vec!["first", "second"]
        );
    }

    #[test]
    fn section_detail_groups_fields_for_dashboard_consumption() {
        let detail = UrlTestDetail {
            id: Uuid::new_v4(),
            started_at: Utc::now(),
            completed_at: None,
            requested_url: "https://example.com".into(),
            final_url: Some("https://www.example.com".into()),
            status: "completed".into(),
            page_load_strategy: "browser".into(),
            browser_engine: Some("chromium".into()),
            browser_version: Some("123.0".into()),
            user_agent: None,
            primary_origin: Some("https://www.example.com".into()),
            observed_protocol_primary_load: Some("h3".into()),
            advertised_alt_svc: Some("h3=\":443\"".into()),
            validated_http_versions: vec!["h2".into(), "h3".into()],
            tls_version: Some("TLS 1.3".into()),
            cipher_suite: Some("TLS_AES_128_GCM_SHA256".into()),
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
            peak_concurrent_connections: Some(3),
            redirect_count: 1,
            failure_count: 0,
            har_path: Some("/tmp/url.har".into()),
            pcap_path: None,
            pcap_summary: Some(UrlPacketCaptureSummaryView {
                mode: "tester".into(),
                interface: "lo".into(),
                capture_path: "/tmp/packet-capture.pcapng".into(),
                total_packets: 42,
                capture_status: "captured".into(),
                note: None,
                warnings: vec![],
                tcp_packets: 10,
                udp_packets: 20,
                quic_packets: 12,
                http_packets: 5,
                dns_packets: 2,
                retransmissions: 1,
                duplicate_acks: 0,
                resets: 0,
                observed_quic: true,
                observed_tcp_only: false,
                observed_mixed_transport: true,
                capture_may_be_ambiguous: false,
            }),
            capture_errors: vec!["pcap unavailable".into()],
            environment_notes: Some("linux runner".into()),
            origin_summaries: vec![],
            connection_summary: None,
            resources: vec![UrlTestResourceRow {
                resource_url: "https://www.example.com/app.js".into(),
                origin: "https://www.example.com".into(),
                resource_type: "script".into(),
                mime_type: None,
                status_code: Some(200),
                protocol: Some("h3".into()),
                transfer_size: Some(2048),
                encoded_body_size: Some(1800),
                decoded_body_size: Some(4096),
                duration_ms: Some(12.0),
                connection_id: None,
                reused_connection: None,
                initiator_type: None,
                from_cache: Some(false),
                redirected: Some(false),
                failed: false,
            }],
            protocol_runs: vec![UrlTestProtocolRunRow {
                protocol_mode: "h3".into(),
                run_number: 1,
                attempt_type: "probe".into(),
                observed_protocol: Some("h3".into()),
                fallback_occurred: Some(false),
                succeeded: true,
                status_code: Some(200),
                ttfb_ms: Some(55.0),
                total_ms: Some(300.0),
                failure_reason: None,
                error: None,
            }],
        };

        let sectioned = section_detail(detail);
        assert_eq!(sectioned.overview.status, "completed");
        assert_eq!(sectioned.protocol.validated_http_versions, vec!["h2", "h3"]);
        assert_eq!(sectioned.tls.alpn.as_deref(), Some("h3"));
        assert_eq!(sectioned.artifacts.capture_errors, vec!["pcap unavailable"]);
        assert_eq!(
            sectioned
                .artifacts
                .pcap_summary
                .as_ref()
                .map(|s| s.total_packets),
            Some(42)
        );
        assert_eq!(sectioned.origin_summaries.len(), 1);
        assert_eq!(
            sectioned
                .connection_summary
                .as_ref()
                .and_then(|s| s.peak_origin_request_count),
            Some(1)
        );
        assert_eq!(sectioned.resources.len(), 1);
        assert_eq!(sectioned.protocol_runs.len(), 1);
    }
}
