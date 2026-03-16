use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct RunSummary {
    pub run_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub target_url: String,
    pub target_host: String,
    pub modes: String,
    pub total_runs: i32,
    pub success_count: i32,
    pub failure_count: i32,
    pub client_os: String,
    pub client_version: String,
}

pub async fn list(
    client: &Client,
    target_host: Option<&str>,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<RunSummary>> {
    let rows = if let Some(host) = target_host {
        client
            .query(
                "SELECT RunId, StartedAt, FinishedAt, TargetUrl, TargetHost, Modes,
                        TotalRuns, SuccessCount, FailureCount, ClientOs, ClientVersion
                 FROM TestRun WHERE TargetHost = $1
                 ORDER BY StartedAt DESC LIMIT $2 OFFSET $3",
                &[&host, &limit, &offset],
            )
            .await?
    } else {
        client
            .query(
                "SELECT RunId, StartedAt, FinishedAt, TargetUrl, TargetHost, Modes,
                        TotalRuns, SuccessCount, FailureCount, ClientOs, ClientVersion
                 FROM TestRun ORDER BY StartedAt DESC LIMIT $1 OFFSET $2",
                &[&limit, &offset],
            )
            .await?
    };

    Ok(rows
        .iter()
        .map(|r| RunSummary {
            run_id: r.get("runid"),
            started_at: r.get("startedat"),
            finished_at: r.get("finishedat"),
            target_url: r.get("targeturl"),
            target_host: r.get("targethost"),
            modes: r.get("modes"),
            total_runs: r.get("totalruns"),
            success_count: r.get("successcount"),
            failure_count: r.get("failurecount"),
            client_os: r.get("clientos"),
            client_version: r.get("clientversion"),
        })
        .collect())
}

/// Return full attempts with all sub-results (DNS, TCP, TLS, HTTP, UDP, errors)
/// in the same LiveAttempt JSON format used by the WebSocket stream.
pub async fn get_attempts(client: &Client, run_id: &Uuid) -> anyhow::Result<serde_json::Value> {
    // Fetch all attempts (include extra_json if the column exists)
    let attempt_rows = client
        .query(
            "SELECT AttemptId, Protocol, SequenceNum, StartedAt, FinishedAt,
                    Success, ErrorMessage, RetryCount,
                    extra_json
             FROM RequestAttempt WHERE RunId = $1
             ORDER BY SequenceNum",
            &[run_id],
        )
        .await
        .or_else(|_| {
            // Fallback: query without extra_json (older schema)
            futures::executor::block_on(client.query(
                "SELECT AttemptId, Protocol, SequenceNum, StartedAt, FinishedAt,
                        Success, ErrorMessage, RetryCount
                 FROM RequestAttempt WHERE RunId = $1
                 ORDER BY SequenceNum",
                &[run_id],
            ))
        })?;

    let mut attempts: Vec<serde_json::Value> = Vec::new();

    for r in &attempt_rows {
        let attempt_id: Uuid = r.get("attemptid");

        // If extra_json is available, use it directly (contains full browser/pageload data)
        if let Ok(Some(extra)) = r.try_get::<_, Option<serde_json::Value>>("extra_json") {
            if extra.is_object() {
                let mut attempt = extra;
                // Ensure attempt_id and run_id are strings (not UUIDs)
                attempt["attempt_id"] = serde_json::json!(attempt_id.to_string());
                attempt["run_id"] = serde_json::json!(run_id.to_string());
                attempts.push(attempt);
                continue;
            }
        }

        // Fallback: build from relational sub-tables
        let mut attempt = serde_json::json!({
            "attempt_id": attempt_id.to_string(),
            "run_id": run_id.to_string(),
            "protocol": r.get::<_, String>("protocol"),
            "sequence_num": r.get::<_, i32>("sequencenum"),
            "started_at": r.get::<_, DateTime<Utc>>("startedat").to_rfc3339(),
            "finished_at": r.get::<_, Option<DateTime<Utc>>>("finishedat").map(|d| d.to_rfc3339()),
            "success": r.get::<_, bool>("success"),
            "retry_count": r.get::<_, i32>("retrycount"),
        });

        // DNS sub-result
        if let Some(dns) = client
            .query_opt(
                "SELECT QueryName, ResolvedIps, DurationMs FROM DnsResult WHERE AttemptId = $1",
                &[&attempt_id],
            )
            .await?
        {
            let ips_str: String = dns.get("resolvedips");
            let resolved_ips: Vec<&str> = ips_str.split(',').map(|s| s.trim()).collect();
            attempt["dns"] = serde_json::json!({
                "duration_ms": dns.get::<_, f64>("durationms"),
                "query_name": dns.get::<_, String>("queryname"),
                "resolved_ips": resolved_ips,
            });
        }

        // TCP sub-result
        if let Some(tcp) = client
            .query_opt(
                "SELECT RemoteAddr, ConnectDurationMs, MssBytesEstimate, RttEstimateMs
                 FROM TcpResult WHERE AttemptId = $1",
                &[&attempt_id],
            )
            .await?
        {
            attempt["tcp"] = serde_json::json!({
                "connect_duration_ms": tcp.get::<_, f64>("connectdurationms"),
                "remote_addr": tcp.get::<_, String>("remoteaddr"),
            });
        }

        // TLS sub-result
        if let Some(tls) = client
            .query_opt(
                "SELECT ProtocolVersion, CipherSuite, AlpnNegotiated, HandshakeDurationMs
                 FROM TlsResult WHERE AttemptId = $1",
                &[&attempt_id],
            )
            .await?
        {
            attempt["tls"] = serde_json::json!({
                "handshake_duration_ms": tls.get::<_, f64>("handshakedurationms"),
                "protocol_version": tls.get::<_, String>("protocolversion"),
                "cipher_suite": tls.get::<_, String>("ciphersuite"),
            });
        }

        // HTTP sub-result
        if let Some(http) = client
            .query_opt(
                "SELECT StatusCode, TtfbMs, TotalDurationMs, NegotiatedVersion,
                        ThroughputMbps, PayloadBytes, BodySizeBytes, HeadersSizeBytes
                 FROM HttpResult WHERE AttemptId = $1",
                &[&attempt_id],
            )
            .await?
        {
            let mut http_json = serde_json::json!({
                "status_code": http.get::<_, i32>("statuscode"),
                "ttfb_ms": http.get::<_, f64>("ttfbms"),
                "total_duration_ms": http.get::<_, f64>("totaldurationms"),
                "negotiated_version": http.get::<_, String>("negotiatedversion"),
                "body_size_bytes": http.get::<_, i32>("bodysizebytes"),
                "headers_size_bytes": http.get::<_, i32>("headerssizebytes"),
            });
            if let Some(tp) = http.get::<_, Option<f64>>("throughputmbps") {
                http_json["throughput_mbps"] = serde_json::json!(tp);
            }
            if let Some(pb) = http.get::<_, Option<i64>>("payloadbytes") {
                http_json["payload_bytes"] = serde_json::json!(pb);
            }
            attempt["http"] = http_json;
        }

        // UDP sub-result
        if let Some(udp) = client
            .query_opt(
                "SELECT ProbeCount, SuccessCount, LossPercent, RttMinMs, RttAvgMs, RttP95Ms, JitterMs
                 FROM UdpResult WHERE AttemptId = $1",
                &[&attempt_id],
            )
            .await?
        {
            attempt["udp"] = serde_json::json!({
                "rtt_avg_ms": udp.get::<_, f64>("rttavgms"),
                "rtt_min_ms": udp.get::<_, f64>("rttminms"),
                "rtt_p95_ms": udp.get::<_, f64>("rttp95ms"),
                "jitter_ms": udp.get::<_, f64>("jitterms"),
                "loss_percent": udp.get::<_, f64>("losspercent"),
                "probe_count": udp.get::<_, i32>("probecount"),
                "success_count": udp.get::<_, i32>("successcount"),
            });
        }

        // Error sub-result
        if let Some(err_msg) = r.get::<_, Option<String>>("errormessage") {
            if !err_msg.is_empty() {
                // Try to get the detailed error record
                if let Some(err) = client
                    .query_opt(
                        "SELECT ErrorCategory, ErrorMessage, ErrorDetail FROM ErrorRecord WHERE AttemptId = $1",
                        &[&attempt_id],
                    )
                    .await?
                {
                    attempt["error"] = serde_json::json!({
                        "category": err.get::<_, String>("errorcategory"),
                        "message": err.get::<_, String>("errormessage"),
                        "detail": err.get::<_, Option<String>>("errordetail"),
                    });
                } else {
                    attempt["error"] = serde_json::json!({
                        "category": "unknown",
                        "message": err_msg,
                    });
                }
            }
        }

        // Check for page_load and browser results in httpresult
        // (they're stored as regular http results with specific protocol names)
        let protocol: String = r.get("protocol");
        if protocol.starts_with("pageload") && attempt.get("http").is_some() {
            let http = &attempt["http"];
            attempt["page_load"] = serde_json::json!({
                "total_ms": http["total_duration_ms"],
                "ttfb_ms": http["ttfb_ms"],
                "asset_count": 0,
                "assets_fetched": 0,
            });
        }
        if protocol.starts_with("browser") && attempt.get("http").is_some() {
            let http = &attempt["http"];
            attempt["browser"] = serde_json::json!({
                "load_ms": http["total_duration_ms"],
                "ttfb_ms": http["ttfb_ms"],
            });
        }

        attempts.push(attempt);
    }

    Ok(serde_json::Value::Array(attempts))
}
