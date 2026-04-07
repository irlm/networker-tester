/// SQL Server backend using `tiberius`.
///
/// Schema targets the tables defined in `sql/01_CreateDatabase.sql`.
/// Parameterized inserts are used throughout; no stored procedures required.
///
/// # Connection
/// Pass an ADO.NET-style connection string, e.g.:
///   "Server=localhost;Database=NetworkDiagnostics;User Id=sa;Password=Pass!;TrustServerCertificate=true"
use super::DatabaseBackend;
use crate::metrics::{RequestAttempt, TestRun, UrlTestProtocolRun, UrlTestResource, UrlTestRun};
use crate::output::json::{
    benchmark_artifact_if_present, BenchmarkArtifact, BenchmarkCase, BenchmarkDataQuality,
    BenchmarkDiagnostics, BenchmarkEnvironment, BenchmarkLaunch, BenchmarkMetadata,
    BenchmarkMethodology, BenchmarkSample, BenchmarkSummary,
};
use crate::tls_profile::TlsEndpointProfile;
use anyhow::Context;
use async_trait::async_trait;
use serde::Serialize;
use tiberius::{Client, Config, Query};
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncWriteCompatExt;

type SqlClient = Client<tokio_util::compat::Compat<TcpStream>>;

/// SQL Server database backend.
pub struct MssqlBackend {
    client: tokio::sync::Mutex<SqlClient>,
}

impl MssqlBackend {
    /// Connect to SQL Server using an ADO.NET-style connection string.
    pub async fn connect(conn_str: &str) -> anyhow::Result<Self> {
        let config =
            Config::from_ado_string(conn_str).context("Failed to parse connection string")?;
        let tcp = TcpStream::connect(config.get_addr())
            .await
            .context("TCP connect to SQL Server")?;
        tcp.set_nodelay(true)?;
        let client = Client::connect(config, tcp.compat_write())
            .await
            .context("SQL Server handshake")?;
        Ok(Self {
            client: tokio::sync::Mutex::new(client),
        })
    }
}

#[async_trait]
impl DatabaseBackend for MssqlBackend {
    async fn migrate(&self) -> anyhow::Result<()> {
        // SQL Server schema is managed externally via sqlcmd scripts.
        Ok(())
    }

    async fn save(&self, run: &TestRun) -> anyhow::Result<()> {
        let mut c = self.client.lock().await;
        let benchmark_schema_ready = benchmark_schema_installed(&mut c).await?;
        let benchmark_artifact = benchmark_artifact_if_present(run)?;

        c.simple_query("BEGIN TRAN")
            .await
            .context("BEGIN TestRun transaction")?
            .into_results()
            .await
            .context("BEGIN TestRun transaction")?;

        let result = async {
            insert_test_run(run, &mut c).await?;

            for attempt in &run.attempts {
                insert_request_attempt(attempt, &mut c).await?;

                if let Some(dns) = &attempt.dns {
                    insert_dns_result(attempt, dns, &mut c).await?;
                }
                if let Some(tcp) = &attempt.tcp {
                    insert_tcp_result(attempt, tcp, &mut c).await?;
                }
                if let Some(tls) = &attempt.tls {
                    insert_tls_result(attempt, tls, &mut c).await?;
                }
                if let Some(http) = &attempt.http {
                    insert_http_result(attempt, http, &mut c).await?;
                }
                if let Some(udp) = &attempt.udp {
                    insert_udp_result(attempt, udp, &mut c).await?;
                }
                if let Some(err) = &attempt.error {
                    insert_error(attempt, err, &mut c).await?;
                }
                if let Some(st) = &attempt.server_timing {
                    insert_server_timing_result(attempt, st, &mut c).await?;
                }
            }

            if benchmark_schema_ready {
                if let Some(artifact) = &benchmark_artifact {
                    insert_benchmark_artifact(run.run_id, artifact, &mut c).await?;
                }
            } else if benchmark_artifact.is_some() {
                tracing::debug!(
                    "Benchmark schema not installed in SQL Server; skipping benchmark persistence"
                );
            } else {
                tracing::trace!("Run is not benchmark-mode; skipping benchmark persistence");
            }

            Ok::<(), anyhow::Error>(())
        }
        .await;

        match result {
            Ok(()) => {
                c.simple_query("COMMIT TRAN")
                    .await
                    .context("COMMIT TestRun transaction")?
                    .into_results()
                    .await
                    .context("COMMIT TestRun transaction")?;
                Ok(())
            }
            Err(e) => {
                let _ = c.simple_query("ROLLBACK TRAN").await;
                Err(e)
            }
        }
    }

    async fn save_url_test(&self, run: &UrlTestRun) -> anyhow::Result<()> {
        let mut c = self.client.lock().await;
        c.simple_query("BEGIN TRAN")
            .await
            .context("BEGIN UrlTest transaction")?
            .into_results()
            .await
            .context("BEGIN UrlTest transaction")?;

        let result = async {
            insert_url_test_run(run, &mut c).await?;
            for resource in &run.resources {
                insert_url_test_resource(run.id, resource, &mut c).await?;
            }
            for probe in &run.protocol_runs {
                insert_url_test_protocol_run(run.id, probe, &mut c).await?;
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;

        match result {
            Ok(()) => {
                c.simple_query("COMMIT TRAN")
                    .await
                    .context("COMMIT UrlTest transaction")?
                    .into_results()
                    .await
                    .context("COMMIT UrlTest transaction")?;
                Ok(())
            }
            Err(e) => {
                let _ = c.simple_query("ROLLBACK TRAN").await;
                Err(e)
            }
        }
    }

    async fn save_tls_profile(
        &self,
        _run: &TlsEndpointProfile,
        _project_id: Option<&str>,
    ) -> anyhow::Result<uuid::Uuid> {
        anyhow::bail!("SQL Server TLS profile persistence not yet implemented")
    }

    async fn ping(&self) -> anyhow::Result<()> {
        let mut c = self.client.lock().await;
        c.simple_query("SELECT 1")
            .await
            .context("SQL Server ping")?;
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Insert helpers
// ─────────────────────────────────────────────────────────────────────────────

async fn insert_test_run(run: &TestRun, c: &mut SqlClient) -> anyhow::Result<()> {
    let run_id = run.run_id.to_string();
    let modes = run.modes.join(",");
    let started = run.started_at.naive_utc();
    let finished = run.finished_at.map(|t| t.naive_utc());

    let mut q = Query::new(
        "INSERT INTO dbo.TestRun (
            RunId, StartedAt, FinishedAt, TargetUrl, TargetHost,
            Modes, TotalRuns, Concurrency, TimeoutMs,
            ClientOs, ClientVersion, SuccessCount, FailureCount
         ) VALUES (
            @P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9,@P10,@P11,@P12,@P13
         )",
    );
    q.bind(run_id.as_str());
    q.bind(started);
    q.bind(finished);
    q.bind(run.target_url.as_str());
    q.bind(run.target_host.as_str());
    q.bind(modes.as_str());
    q.bind(run.total_runs as i32);
    q.bind(run.concurrency as i32);
    q.bind(run.timeout_ms as i64);
    q.bind(run.client_os.as_str());
    q.bind(run.client_version.as_str());
    q.bind(run.success_count() as i32);
    q.bind(run.failure_count() as i32);
    q.execute(c).await.context("INSERT TestRun")?;
    Ok(())
}

fn to_json_string<T: Serialize>(value: &T, label: &str) -> anyhow::Result<String> {
    serde_json::to_string(value).context(format!("serialize {label}"))
}

async fn benchmark_schema_installed(c: &mut SqlClient) -> anyhow::Result<bool> {
    let row = c
        .query(
            "SELECT CASE
                WHEN OBJECT_ID(N'dbo.BenchmarkRun') IS NOT NULL
                 AND OBJECT_ID(N'dbo.BenchmarkLaunch') IS NOT NULL
                 AND OBJECT_ID(N'dbo.BenchmarkEnvironment') IS NOT NULL
                 AND OBJECT_ID(N'dbo.BenchmarkDataQuality') IS NOT NULL
                 AND OBJECT_ID(N'dbo.BenchmarkCase') IS NOT NULL
                 AND OBJECT_ID(N'dbo.BenchmarkSample') IS NOT NULL
                 AND OBJECT_ID(N'dbo.BenchmarkSummary') IS NOT NULL
                THEN 1 ELSE 0 END",
            &[],
        )
        .await
        .context("query benchmark schema readiness")?
        .into_row()
        .await
        .context("read benchmark schema readiness row")?;

    let installed = row.and_then(|row| row.get::<i32, _>(0)).unwrap_or(0);
    Ok(installed == 1)
}

async fn insert_benchmark_artifact(
    run_id: uuid::Uuid,
    artifact: &BenchmarkArtifact,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    insert_benchmark_run(
        run_id,
        &artifact.metadata,
        &artifact.methodology,
        &artifact.diagnostics,
        &artifact.summary,
        c,
    )
    .await?;
    insert_benchmark_environment(run_id, &artifact.environment, c).await?;
    insert_benchmark_data_quality(run_id, &artifact.data_quality, c).await?;
    for launch in &artifact.launches {
        insert_benchmark_launch(run_id, launch, c).await?;
    }

    for case in &artifact.cases {
        insert_benchmark_case(run_id, case, c).await?;
    }
    for sample in &artifact.samples {
        insert_benchmark_sample(run_id, sample, c).await?;
    }
    for summary in &artifact.summaries {
        insert_benchmark_summary(run_id, summary, c).await?;
    }

    Ok(())
}

async fn insert_benchmark_launch(
    run_id: uuid::Uuid,
    launch: &BenchmarkLaunch,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let run_id = run_id.to_string();
    let phases_json = to_json_string(&launch.phases_present, "BenchmarkLaunch.phases_present")?;

    let mut q = Query::new(
        "INSERT INTO dbo.BenchmarkLaunch (
            BenchmarkRunId, LaunchIndex, Scenario, PrimaryPhase, StartedAt, FinishedAt,
            SampleCount, PrimarySampleCount, WarmupSampleCount, SuccessCount, FailureCount,
            PhasesJson
         ) VALUES (
            @P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9,@P10,@P11,@P12
         )",
    );
    q.bind(run_id.as_str());
    q.bind(launch.launch_index as i32);
    q.bind(launch.scenario.as_str());
    q.bind(launch.primary_phase.as_str());
    q.bind(launch.started_at.naive_utc());
    q.bind(launch.finished_at.map(|value| value.naive_utc()));
    q.bind(launch.sample_count as i64);
    q.bind(launch.primary_sample_count as i64);
    q.bind(launch.warmup_sample_count as i64);
    q.bind(launch.success_count as i64);
    q.bind(launch.failure_count as i64);
    q.bind(phases_json.as_str());
    q.execute(c).await.context("INSERT BenchmarkLaunch")?;
    Ok(())
}

async fn insert_benchmark_run(
    run_id: uuid::Uuid,
    metadata: &BenchmarkMetadata,
    methodology: &BenchmarkMethodology,
    diagnostics: &BenchmarkDiagnostics,
    aggregate_summary: &BenchmarkSummary,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let run_id = run_id.to_string();
    let modes = metadata.modes.join(",");
    let methodology_json = to_json_string(methodology, "BenchmarkMethodology")?;
    let diagnostics_json = to_json_string(diagnostics, "BenchmarkDiagnostics")?;
    let aggregate_summary_json = to_json_string(aggregate_summary, "BenchmarkSummary")?;

    let mut q = Query::new(
        "INSERT INTO dbo.BenchmarkRun (
            BenchmarkRunId, ContractVersion, GeneratedAt, Source, TargetUrl, TargetHost,
            Modes, TotalRuns, Concurrency, TimeoutMs, ClientOs, ClientVersion,
            MethodologyJson, DiagnosticsJson, AggregateSummaryJson
         ) VALUES (
            @P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9,@P10,@P11,@P12,@P13,@P14,@P15
         )",
    );
    q.bind(run_id.as_str());
    q.bind(metadata.contract_version.as_str());
    q.bind(metadata.generated_at.naive_utc());
    q.bind(metadata.source.as_str());
    q.bind(metadata.target_url.as_str());
    q.bind(metadata.target_host.as_str());
    q.bind(modes.as_str());
    q.bind(metadata.total_runs as i32);
    q.bind(metadata.concurrency as i32);
    q.bind(metadata.timeout_ms as i64);
    q.bind(metadata.client_os.as_str());
    q.bind(metadata.client_version.as_str());
    q.bind(methodology_json.as_str());
    q.bind(diagnostics_json.as_str());
    q.bind(aggregate_summary_json.as_str());
    q.execute(c).await.context("INSERT BenchmarkRun")?;
    Ok(())
}

async fn insert_benchmark_environment(
    run_id: uuid::Uuid,
    environment: &BenchmarkEnvironment,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let run_id = run_id.to_string();
    let client_info_json = environment
        .client_info
        .as_ref()
        .map(|value| to_json_string(value, "BenchmarkEnvironment.client_info"))
        .transpose()?;
    let server_info_json = environment
        .server_info
        .as_ref()
        .map(|value| to_json_string(value, "BenchmarkEnvironment.server_info"))
        .transpose()?;
    let network_baseline_json = environment
        .network_baseline
        .as_ref()
        .map(|value| to_json_string(value, "BenchmarkEnvironment.network_baseline"))
        .transpose()?;
    let environment_json = to_json_string(environment, "BenchmarkEnvironment")?;

    let mut q = Query::new(
        "INSERT INTO dbo.BenchmarkEnvironment (
            BenchmarkRunId, ClientInfoJson, ServerInfoJson, NetworkBaselineJson,
            PacketCaptureEnabled, EnvironmentJson
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6)",
    );
    q.bind(run_id.as_str());
    q.bind(client_info_json.as_deref());
    q.bind(server_info_json.as_deref());
    q.bind(network_baseline_json.as_deref());
    q.bind(environment.packet_capture_enabled);
    q.bind(environment_json.as_str());
    q.execute(c).await.context("INSERT BenchmarkEnvironment")?;
    Ok(())
}

async fn insert_benchmark_data_quality(
    run_id: uuid::Uuid,
    quality: &BenchmarkDataQuality,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let run_id = run_id.to_string();
    let warnings_json = to_json_string(&quality.warnings, "BenchmarkDataQuality.warnings")?;
    let quality_json = to_json_string(quality, "BenchmarkDataQuality")?;

    let mut q = Query::new(
        "INSERT INTO dbo.BenchmarkDataQuality (
            BenchmarkRunId, NoiseLevel, SampleStabilityCv, Sufficiency,
            PublicationReady, WarningsJson, QualityJson
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6,@P7)",
    );
    q.bind(run_id.as_str());
    q.bind(quality.noise_level.as_str());
    q.bind(quality.sample_stability_cv);
    q.bind(quality.sufficiency.as_str());
    q.bind(quality.publication_ready);
    q.bind(warnings_json.as_str());
    q.bind(quality_json.as_str());
    q.execute(c).await.context("INSERT BenchmarkDataQuality")?;
    Ok(())
}

async fn insert_benchmark_case(
    run_id: uuid::Uuid,
    case: &BenchmarkCase,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let run_id = run_id.to_string();
    let case_json = to_json_string(case, "BenchmarkCase")?;

    let mut q = Query::new(
        "INSERT INTO dbo.BenchmarkCase (
            BenchmarkRunId, CaseId, Protocol, PayloadBytes, HttpStack,
            MetricName, MetricUnit, HigherIsBetter, CaseJson
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9)",
    );
    q.bind(run_id.as_str());
    q.bind(case.id.as_str());
    q.bind(case.protocol.as_str());
    q.bind(case.payload_bytes.map(|value| value as i64));
    q.bind(case.http_stack.as_deref());
    q.bind(case.metric_name.as_str());
    q.bind(case.metric_unit.as_str());
    q.bind(case.higher_is_better);
    q.bind(case_json.as_str());
    q.execute(c).await.context("INSERT BenchmarkCase")?;
    Ok(())
}

async fn insert_benchmark_sample(
    run_id: uuid::Uuid,
    sample: &BenchmarkSample,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let attempt_id = sample.attempt_id.to_string();
    let run_id = run_id.to_string();
    let sample_json = to_json_string(sample, "BenchmarkSample")?;

    let mut q = Query::new(
        "INSERT INTO dbo.BenchmarkSample (
            AttemptId, BenchmarkRunId, CaseId, LaunchIndex, Phase, IterationIndex,
            Success, RetryCount, InclusionStatus, MetricValue, MetricUnit, StartedAt,
            FinishedAt, TotalDurationMs, TtfbMs, SampleJson
         ) VALUES (
            @P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9,@P10,@P11,@P12,@P13,@P14,@P15,@P16
         )",
    );
    q.bind(attempt_id.as_str());
    q.bind(run_id.as_str());
    q.bind(sample.case_id.as_str());
    q.bind(sample.launch_index as i32);
    q.bind(sample.phase.as_str());
    q.bind(sample.iteration_index as i32);
    q.bind(sample.success);
    q.bind(sample.retry_count as i32);
    q.bind(sample.inclusion_status.as_str());
    q.bind(sample.metric_value);
    q.bind(sample.metric_unit.as_str());
    q.bind(sample.started_at.naive_utc());
    q.bind(sample.finished_at.map(|value| value.naive_utc()));
    q.bind(sample.total_duration_ms);
    q.bind(sample.ttfb_ms);
    q.bind(sample_json.as_str());
    q.execute(c).await.context("INSERT BenchmarkSample")?;
    Ok(())
}

async fn insert_benchmark_summary(
    run_id: uuid::Uuid,
    summary: &BenchmarkSummary,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let run_id = run_id.to_string();
    let summary_json = to_json_string(summary, "BenchmarkSummary")?;

    let mut q = Query::new(
        "INSERT INTO dbo.BenchmarkSummary (
            BenchmarkRunId, CaseId, Protocol, PayloadBytes, HttpStack, MetricName,
            MetricUnit, HigherIsBetter, SampleCount, IncludedSampleCount,
            ExcludedSampleCount, SuccessCount, FailureCount, TotalRequests, ErrorCount,
            BytesTransferred, WallTimeMs, Rps, Min, Mean, P5, P25, P50, P75, P95, P99,
            P999, Max, Stddev, LatencyMeanMs, LatencyP50Ms, LatencyP99Ms,
            LatencyP999Ms, LatencyMaxMs, SummaryJson
         ) VALUES (
            @P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9,@P10,@P11,@P12,@P13,@P14,@P15,@P16,@P17,
            @P18,@P19,@P20,@P21,@P22,@P23,@P24,@P25,@P26,@P27,@P28,@P29,@P30,@P31,@P32,
            @P33,@P34,@P35
         )",
    );
    q.bind(run_id.as_str());
    q.bind(summary.case_id.as_str());
    q.bind(summary.protocol.as_str());
    q.bind(summary.payload_bytes.map(|value| value as i64));
    q.bind(summary.http_stack.as_deref());
    q.bind(summary.metric_name.as_str());
    q.bind(summary.metric_unit.as_str());
    q.bind(summary.higher_is_better);
    q.bind(summary.sample_count as i64);
    q.bind(summary.included_sample_count as i64);
    q.bind(summary.excluded_sample_count as i64);
    q.bind(summary.success_count as i64);
    q.bind(summary.failure_count as i64);
    q.bind(summary.total_requests as i64);
    q.bind(summary.error_count as i64);
    q.bind(summary.bytes_transferred as i64);
    q.bind(summary.wall_time_ms);
    q.bind(summary.rps);
    q.bind(summary.min);
    q.bind(summary.mean);
    q.bind(summary.p5);
    q.bind(summary.p25);
    q.bind(summary.p50);
    q.bind(summary.p75);
    q.bind(summary.p95);
    q.bind(summary.p99);
    q.bind(summary.p999);
    q.bind(summary.max);
    q.bind(summary.stddev);
    q.bind(summary.latency_mean_ms);
    q.bind(summary.latency_p50_ms);
    q.bind(summary.latency_p99_ms);
    q.bind(summary.latency_p999_ms);
    q.bind(summary.latency_max_ms);
    q.bind(summary_json.as_str());
    q.execute(c).await.context("INSERT BenchmarkSummary")?;
    Ok(())
}

async fn insert_url_test_run(run: &UrlTestRun, c: &mut SqlClient) -> anyhow::Result<()> {
    let validated_http_versions = run.validated_http_versions.join(",");
    let capture_errors = if run.capture_errors.is_empty() {
        None
    } else {
        Some(run.capture_errors.join("\n"))
    };
    let pcap_summary_json = run
        .pcap_summary
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .context("serialize UrlPacketCaptureSummary")?;
    let status = run.status.to_string();
    let page_load_strategy = serde_json::to_value(&run.page_load_strategy)?
        .as_str()
        .unwrap_or("browser")
        .to_string();

    let mut q = Query::new(
        "INSERT INTO dbo.UrlTestRun (
            Id, StartedAt, CompletedAt, RequestedUrl, FinalUrl, Status, PageLoadStrategy,
            BrowserEngine, BrowserVersion, UserAgent, PrimaryOrigin, ObservedProtocolPrimaryLoad,
            AdvertisedAltSvc, ValidatedHttpVersions, TlsVersion, CipherSuite, Alpn,
            DnsMs, ConnectMs, HandshakeMs, TtfbMs, DomContentLoadedMs, LoadEventMs,
            NetworkIdleMs, CaptureEndMs, TotalRequests, TotalTransferBytes,
            PeakConcurrentConnections, RedirectCount, FailureCount, HarPath, PcapPath,
            PcapSummaryJson, CaptureErrors, EnvironmentNotes
        ) VALUES (
            @P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9,@P10,@P11,@P12,@P13,@P14,@P15,@P16,@P17,
            @P18,@P19,@P20,@P21,@P22,@P23,@P24,@P25,@P26,@P27,@P28,@P29,@P30,@P31,@P32,@P33,@P34,@P35
        )",
    );
    q.bind(run.id.to_string());
    q.bind(run.started_at.naive_utc());
    q.bind(run.completed_at.map(|t| t.naive_utc()));
    q.bind(run.requested_url.as_str());
    q.bind(run.final_url.as_deref());
    q.bind(status.as_str());
    q.bind(page_load_strategy.as_str());
    q.bind(run.browser_engine.as_deref());
    q.bind(run.browser_version.as_deref());
    q.bind(run.user_agent.as_deref());
    q.bind(run.primary_origin.as_deref());
    q.bind(run.observed_protocol_primary_load.as_deref());
    q.bind(run.advertised_alt_svc.as_deref());
    q.bind(validated_http_versions.as_str());
    q.bind(run.tls_version.as_deref());
    q.bind(run.cipher_suite.as_deref());
    q.bind(run.alpn.as_deref());
    q.bind(run.dns_ms);
    q.bind(run.connect_ms);
    q.bind(run.handshake_ms);
    q.bind(run.ttfb_ms);
    q.bind(run.dom_content_loaded_ms);
    q.bind(run.load_event_ms);
    q.bind(run.network_idle_ms);
    q.bind(run.capture_end_ms);
    q.bind(run.total_requests as i32);
    q.bind(run.total_transfer_bytes as i64);
    q.bind(run.peak_concurrent_connections.map(|v| v as i32));
    q.bind(run.redirect_count as i32);
    q.bind(run.failure_count as i32);
    q.bind(run.har_path.as_deref());
    q.bind(run.pcap_path.as_deref());
    q.bind(pcap_summary_json.as_deref());
    q.bind(capture_errors.as_deref());
    q.bind(run.environment_notes.as_deref());
    q.execute(c).await.context("INSERT UrlTestRun")?;
    Ok(())
}

async fn insert_url_test_resource(
    run_id: uuid::Uuid,
    r: &UrlTestResource,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let mut q = Query::new(
        "INSERT INTO dbo.UrlTestResource (
            Id, UrlTestRunId, ResourceUrl, Origin, ResourceType, MimeType, StatusCode,
            Protocol, TransferSize, EncodedBodySize, DecodedBodySize, DurationMs,
            ConnectionId, ReusedConnection, InitiatorType, FromCache, Redirected, Failed
        ) VALUES (
            @P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9,@P10,@P11,@P12,@P13,@P14,@P15,@P16,@P17,@P18
        )",
    );
    q.bind(uuid::Uuid::new_v4().to_string());
    q.bind(run_id.to_string());
    q.bind(r.resource_url.as_str());
    q.bind(r.origin.as_str());
    q.bind(r.resource_type.as_str());
    q.bind(r.mime_type.as_deref());
    q.bind(r.status_code.map(|v| v as i32));
    q.bind(r.protocol.as_deref());
    q.bind(r.transfer_size.map(|v| v as i64));
    q.bind(r.encoded_body_size.map(|v| v as i64));
    q.bind(r.decoded_body_size.map(|v| v as i64));
    q.bind(r.duration_ms);
    q.bind(r.connection_id.as_deref());
    q.bind(r.reused_connection);
    q.bind(r.initiator_type.as_deref());
    q.bind(r.from_cache);
    q.bind(r.redirected);
    q.bind(r.failed);
    q.execute(c).await.context("INSERT UrlTestResource")?;
    Ok(())
}

async fn insert_url_test_protocol_run(
    run_id: uuid::Uuid,
    p: &UrlTestProtocolRun,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let attempt_type = serde_json::to_value(&p.attempt_type)?
        .as_str()
        .unwrap_or("probe")
        .to_string();
    let mut q = Query::new(
        "INSERT INTO dbo.UrlTestProtocolRun (
            Id, UrlTestRunId, ProtocolMode, RunNumber, AttemptType, ObservedProtocol,
            FallbackOccurred, Succeeded, StatusCode, TtfbMs, TotalMs, FailureReason, Error
        ) VALUES (
            @P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9,@P10,@P11,@P12,@P13
        )",
    );
    q.bind(uuid::Uuid::new_v4().to_string());
    q.bind(run_id.to_string());
    q.bind(p.protocol_mode.as_str());
    q.bind(p.run_number as i32);
    q.bind(attempt_type.as_str());
    q.bind(p.observed_protocol.as_deref());
    q.bind(p.fallback_occurred);
    q.bind(p.succeeded);
    q.bind(p.status_code.map(|v| v as i32));
    q.bind(p.ttfb_ms);
    q.bind(p.total_ms);
    q.bind(p.failure_reason.as_deref());
    q.bind(p.error.as_deref());
    q.execute(c).await.context("INSERT UrlTestProtocolRun")?;
    Ok(())
}

async fn insert_request_attempt(a: &RequestAttempt, c: &mut SqlClient) -> anyhow::Result<()> {
    let attempt_id = a.attempt_id.to_string();
    let run_id = a.run_id.to_string();
    let protocol = a.protocol.to_string();
    let started = a.started_at.naive_utc();
    let finished = a.finished_at.map(|t| t.naive_utc());
    let err_msg = a.error.as_ref().map(|e| e.message.as_str());

    let mut q = Query::new(
        "INSERT INTO dbo.RequestAttempt (
            AttemptId, RunId, Protocol, SequenceNum,
            StartedAt, FinishedAt, Success, ErrorMessage, RetryCount
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9)",
    );
    q.bind(attempt_id.as_str());
    q.bind(run_id.as_str());
    q.bind(protocol.as_str());
    q.bind(a.sequence_num as i32);
    q.bind(started);
    q.bind(finished);
    q.bind(a.success);
    q.bind(err_msg);
    q.bind(a.retry_count as i32);
    q.execute(c).await.context("INSERT RequestAttempt")?;
    Ok(())
}

async fn insert_dns_result(
    a: &RequestAttempt,
    dns: &crate::metrics::DnsResult,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let attempt_id = a.attempt_id.to_string();
    let ips = dns.resolved_ips.join(",");
    let started = dns.started_at.naive_utc();

    let mut q = Query::new(
        "INSERT INTO dbo.DnsResult (
            DnsId, AttemptId, QueryName, ResolvedIPs,
            DurationMs, StartedAt, Success
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6,@P7)",
    );
    q.bind(id.as_str());
    q.bind(attempt_id.as_str());
    q.bind(dns.query_name.as_str());
    q.bind(ips.as_str());
    q.bind(dns.duration_ms);
    q.bind(started);
    q.bind(dns.success);
    q.execute(c).await.context("INSERT DnsResult")?;
    Ok(())
}

async fn insert_tcp_result(
    a: &RequestAttempt,
    tcp: &crate::metrics::TcpResult,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let attempt_id = a.attempt_id.to_string();
    let started = tcp.started_at.naive_utc();

    let mut q = Query::new(
        "INSERT INTO dbo.TcpResult (
            TcpId, AttemptId, LocalAddr, RemoteAddr,
            ConnectDurationMs, AttemptCount, StartedAt, Success,
            MssBytesEstimate, RttEstimateMs,
            Retransmits, TotalRetrans, SndCwnd, SndSsthresh,
            RttVarianceMs, RcvSpace, SegsOut, SegsIn,
            CongestionAlgorithm, DeliveryRateBps, MinRttMs
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9,@P10,
                   @P11,@P12,@P13,@P14,@P15,@P16,@P17,@P18,
                   @P19,@P20,@P21)",
    );
    q.bind(id.as_str());
    q.bind(attempt_id.as_str());
    q.bind(tcp.local_addr.as_deref());
    q.bind(tcp.remote_addr.as_str());
    q.bind(tcp.connect_duration_ms);
    q.bind(tcp.attempt_count as i32);
    q.bind(started);
    q.bind(tcp.success);
    q.bind(tcp.mss_bytes.map(|v| v as i32));
    q.bind(tcp.rtt_estimate_ms);
    // Extended kernel stats (nullable)
    q.bind(tcp.retransmits.map(|v| v as i64));
    q.bind(tcp.total_retrans.map(|v| v as i64));
    q.bind(tcp.snd_cwnd.map(|v| v as i64));
    q.bind(tcp.snd_ssthresh.map(|v| v as i64));
    q.bind(tcp.rtt_variance_ms);
    q.bind(tcp.rcv_space.map(|v| v as i64));
    q.bind(tcp.segs_out.map(|v| v as i64));
    q.bind(tcp.segs_in.map(|v| v as i64));
    // New fields (07_MoreTcpStats.sql)
    q.bind(tcp.congestion_algorithm.as_deref());
    q.bind(tcp.delivery_rate_bps.map(|v| v as i64));
    q.bind(tcp.min_rtt_ms);
    q.execute(c).await.context("INSERT TcpResult")?;
    Ok(())
}

async fn insert_server_timing_result(
    a: &RequestAttempt,
    st: &crate::metrics::ServerTimingResult,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let attempt_id = a.attempt_id.to_string();
    let server_ts = st.server_timestamp.map(|t| t.naive_utc());

    let mut q = Query::new(
        "INSERT INTO dbo.ServerTimingResult (
            ServerId, AttemptId, RequestId, ServerTimestamp,
            ClockSkewMs, RecvBodyMs, ProcessingMs, TotalServerMs
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8)",
    );
    q.bind(id.as_str());
    q.bind(attempt_id.as_str());
    q.bind(st.request_id.as_deref());
    q.bind(server_ts);
    q.bind(st.clock_skew_ms);
    q.bind(st.recv_body_ms);
    q.bind(st.processing_ms);
    q.bind(st.total_server_ms);
    q.execute(c).await.context("INSERT ServerTimingResult")?;
    Ok(())
}

async fn insert_tls_result(
    a: &RequestAttempt,
    tls: &crate::metrics::TlsResult,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let attempt_id = a.attempt_id.to_string();
    let started = tls.started_at.naive_utc();
    let expiry = tls.cert_expiry.map(|t| t.naive_utc());

    let mut q = Query::new(
        "INSERT INTO dbo.TlsResult (
            TlsId, AttemptId, ProtocolVersion, CipherSuite,
            AlpnNegotiated, CertSubject, CertIssuer, CertExpiry,
            HandshakeDurationMs, StartedAt, Success
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9,@P10,@P11)",
    );
    q.bind(id.as_str());
    q.bind(attempt_id.as_str());
    q.bind(tls.protocol_version.as_str());
    q.bind(tls.cipher_suite.as_str());
    q.bind(tls.alpn_negotiated.as_deref());
    q.bind(tls.cert_subject.as_deref());
    q.bind(tls.cert_issuer.as_deref());
    q.bind(expiry);
    q.bind(tls.handshake_duration_ms);
    q.bind(started);
    q.bind(tls.success);
    q.execute(c).await.context("INSERT TlsResult")?;
    Ok(())
}

async fn insert_http_result(
    a: &RequestAttempt,
    http: &crate::metrics::HttpResult,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let attempt_id = a.attempt_id.to_string();
    let started = http.started_at.naive_utc();

    let mut q = Query::new(
        "INSERT INTO dbo.HttpResult (
            HttpId, AttemptId, NegotiatedVersion, StatusCode,
            HeadersSizeBytes, BodySizeBytes, TtfbMs,
            TotalDurationMs, RedirectCount, StartedAt,
            PayloadBytes, ThroughputMbps
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9,@P10,@P11,@P12)",
    );
    q.bind(id.as_str());
    q.bind(attempt_id.as_str());
    q.bind(http.negotiated_version.as_str());
    q.bind(http.status_code as i32);
    q.bind(http.headers_size_bytes as i32);
    q.bind(http.body_size_bytes as i32);
    q.bind(http.ttfb_ms);
    q.bind(http.total_duration_ms);
    q.bind(http.redirect_count as i32);
    q.bind(started);
    q.bind(http.payload_bytes as i64);
    q.bind(http.throughput_mbps);
    q.execute(c).await.context("INSERT HttpResult")?;
    Ok(())
}

async fn insert_udp_result(
    a: &RequestAttempt,
    udp: &crate::metrics::UdpResult,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let attempt_id = a.attempt_id.to_string();
    let started = udp.started_at.naive_utc();

    let mut q = Query::new(
        "INSERT INTO dbo.UdpResult (
            UdpId, AttemptId, RemoteAddr, ProbeCount,
            SuccessCount, LossPercent, RttMinMs, RttAvgMs,
            RttP95Ms, JitterMs, StartedAt
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6,@P7,@P8,@P9,@P10,@P11)",
    );
    q.bind(id.as_str());
    q.bind(attempt_id.as_str());
    q.bind(udp.remote_addr.as_str());
    q.bind(udp.probe_count as i32);
    q.bind(udp.success_count as i32);
    q.bind(udp.loss_percent);
    q.bind(udp.rtt_min_ms);
    q.bind(udp.rtt_avg_ms);
    q.bind(udp.rtt_p95_ms);
    q.bind(udp.jitter_ms);
    q.bind(started);
    q.execute(c).await.context("INSERT UdpResult")?;
    Ok(())
}

async fn insert_error(
    a: &RequestAttempt,
    err: &crate::metrics::ErrorRecord,
    c: &mut SqlClient,
) -> anyhow::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let attempt_id = a.attempt_id.to_string();
    let run_id = a.run_id.to_string();
    let category = err.category.to_string();
    let occurred = err.occurred_at.naive_utc();

    let mut q = Query::new(
        "INSERT INTO dbo.ErrorRecord (
            ErrorId, AttemptId, RunId, ErrorCategory, ErrorMessage, ErrorDetail, OccurredAt
         ) VALUES (@P1,@P2,@P3,@P4,@P5,@P6,@P7)",
    );
    q.bind(id.as_str());
    q.bind(attempt_id.as_str());
    q.bind(run_id.as_str());
    q.bind(category.as_str());
    q.bind(err.message.as_str());
    q.bind(err.detail.as_deref());
    q.bind(occurred);
    q.execute(c).await.context("INSERT ErrorRecord")?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::db::test_fixtures::{
        bare_attempt, full_attempt, make_benchmark_run, make_run,
    };
    use tokio::time::{sleep, Duration};
    use uuid::Uuid;

    const DOCUMENTED_V001_SCHEMA: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sql/01_CreateDatabase.sql"
    ));
    const DOCUMENTED_V004_THROUGHPUT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sql/04_AddThroughput.sql"
    ));
    const DOCUMENTED_V005_EXTENDED_TCP: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sql/05_ExtendedTcpStats.sql"
    ));
    const DOCUMENTED_V006_SERVER_TIMING: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sql/06_ServerTiming.sql"
    ));
    const DOCUMENTED_V007_MORE_TCP: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sql/07_MoreTcpStats.sql"
    ));
    const DOCUMENTED_V008_URL_DIAGNOSTICS: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sql/08_UrlDiagnostics.sql"
    ));
    const BENCHMARK_SCHEMA_SQL: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sql/09_BenchmarkSchema.sql"
    ));

    /// Returns `NETWORKER_SQL_CONN` or skips the test (returns None) if unset.
    fn sql_conn() -> Option<String> {
        std::env::var("NETWORKER_SQL_CONN").ok()
    }

    fn connection_string_with_database(base_conn: &str, db_name: &str) -> String {
        let mut parts = Vec::new();
        let mut replaced = false;

        for segment in base_conn.split(';') {
            let trimmed = segment.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Some((key, _)) = trimmed.split_once('=') {
                let normalized = key.trim().to_ascii_lowercase();
                if normalized == "database" || normalized == "initial catalog" {
                    if !replaced {
                        parts.push(format!("Database={db_name}"));
                        replaced = true;
                    }
                    continue;
                }
            }

            parts.push(trimmed.to_string());
        }

        if !replaced {
            parts.push(format!("Database={db_name}"));
        }

        parts.join(";")
    }

    fn render_documented_script(script: &str, db_name: &str) -> String {
        script.replace("NetworkDiagnostics", db_name)
    }

    fn split_sqlcmd_batches(script: &str) -> Vec<String> {
        let mut batches = Vec::new();
        let mut current = String::new();

        for line in script.lines() {
            if line.trim().eq_ignore_ascii_case("GO") {
                if !current.trim().is_empty() {
                    batches.push(current.trim().to_string());
                    current.clear();
                }
                continue;
            }

            current.push_str(line);
            current.push('\n');
        }

        if !current.trim().is_empty() {
            batches.push(current.trim().to_string());
        }

        batches
    }

    async fn try_test_connect(conn_str: &str) -> anyhow::Result<SqlClient> {
        let config = Config::from_ado_string(conn_str).unwrap();
        let tcp = TcpStream::connect(config.get_addr()).await?;
        tcp.set_nodelay(true)?;
        Ok(Client::connect(config, tcp.compat_write()).await?)
    }

    /// Helper: connect directly for test verification queries.
    async fn test_connect(conn_str: &str) -> SqlClient {
        let mut last_err = None;

        for _ in 0..20 {
            match try_test_connect(conn_str).await {
                Ok(client) => return client,
                Err(err) => {
                    last_err = Some(err);
                    sleep(Duration::from_millis(250)).await;
                }
            }
        }

        panic!(
            "failed to connect to SQL Server test database: {}",
            last_err.unwrap()
        );
    }

    async fn execute_sqlcmd_script(
        client: &mut SqlClient,
        script: &str,
        db_name: &str,
    ) -> anyhow::Result<()> {
        let rendered = render_documented_script(script, db_name);
        for batch in split_sqlcmd_batches(&rendered) {
            client.simple_query(batch).await?.into_results().await?;
        }
        Ok(())
    }

    async fn install_documented_schema(
        base_conn: &str,
        db_name: &str,
        benchmark_schema: bool,
    ) -> anyhow::Result<()> {
        let admin_conn = connection_string_with_database(base_conn, "master");
        let mut client = test_connect(&admin_conn).await;

        for script in [
            DOCUMENTED_V001_SCHEMA,
            DOCUMENTED_V004_THROUGHPUT,
            DOCUMENTED_V005_EXTENDED_TCP,
            DOCUMENTED_V006_SERVER_TIMING,
            DOCUMENTED_V007_MORE_TCP,
            DOCUMENTED_V008_URL_DIAGNOSTICS,
        ] {
            execute_sqlcmd_script(&mut client, script, db_name).await?;
        }

        if benchmark_schema {
            execute_sqlcmd_script(&mut client, BENCHMARK_SCHEMA_SQL, db_name).await?;
        }

        Ok(())
    }

    async fn isolated_backend_with_documented_schema(
        base_conn: &str,
        prefix: &str,
        benchmark_schema: bool,
    ) -> (String, MssqlBackend) {
        let db_name = format!("networker_{}_{}", prefix, Uuid::new_v4().simple());
        install_documented_schema(base_conn, &db_name, benchmark_schema)
            .await
            .unwrap();
        let db_conn = connection_string_with_database(base_conn, &db_name);
        let backend = MssqlBackend::connect(&db_conn).await.unwrap();
        (db_conn, backend)
    }

    async fn benchmark_table_exists(client: &mut SqlClient, table: &str) -> bool {
        let row = query_one(
            client,
            &format!("SELECT CASE WHEN OBJECT_ID(N'dbo.{table}') IS NOT NULL THEN 1 ELSE 0 END"),
        )
        .await
        .expect("benchmark table existence row");
        let exists: i32 = row.get(0).unwrap();
        exists == 1
    }

    /// Helper: execute a SELECT, return the first row.
    async fn query_one(client: &mut SqlClient, sql: &str) -> Option<tiberius::Row> {
        let stream = client.query(sql, &[]).await.ok()?;
        let row = stream.into_row().await.ok()?;
        row
    }

    /// Helper: execute a SELECT, return all rows.
    async fn query_all(client: &mut SqlClient, sql: &str) -> Vec<tiberius::Row> {
        let stream = client.query(sql, &[]).await.unwrap();
        stream.into_first_result().await.unwrap()
    }

    #[test]
    fn mssql_benchmark_schema_contains_tables() {
        for table in [
            "dbo.BenchmarkRun",
            "dbo.BenchmarkLaunch",
            "dbo.BenchmarkEnvironment",
            "dbo.BenchmarkDataQuality",
            "dbo.BenchmarkCase",
            "dbo.BenchmarkSample",
            "dbo.BenchmarkSummary",
        ] {
            assert!(
                BENCHMARK_SCHEMA_SQL.contains(&format!("CREATE TABLE {table}")),
                "SQL Server benchmark schema missing CREATE TABLE {table}"
            );
        }
    }

    #[test]
    fn mssql_benchmark_schema_contains_indexes() {
        for idx in [
            "IX_BenchmarkRun_GeneratedAt",
            "IX_BenchmarkLaunch_Phase",
            "IX_BenchmarkCase_Protocol",
            "IX_BenchmarkSample_RunCase",
            "IX_BenchmarkSummary_RunProtocol",
            "IX_BenchmarkDataQuality_PublicationReady",
        ] {
            assert!(
                BENCHMARK_SCHEMA_SQL.contains(idx),
                "SQL Server benchmark schema missing index: {idx}"
            );
        }
    }

    #[test]
    fn mssql_benchmark_schema_starts_with_header_comment() {
        let trimmed = BENCHMARK_SCHEMA_SQL.trim();
        assert!(
            trimmed.starts_with(
                "-- ============================================================================="
            ),
            "SQL Server benchmark schema should start with a header comment"
        );
    }

    /// Basic round-trip: TestRun + bare RequestAttempt (no sub-results).
    #[tokio::test]
    #[ignore = "requires SQL Server -- set NETWORKER_SQL_CONN to enable"]
    async fn db_mssql_insert_round_trip() {
        let base_conn = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let (_db_conn, backend) =
            isolated_backend_with_documented_schema(&base_conn, "insert_round_trip", false).await;
        let run_id = Uuid::new_v4();
        let run = make_run(run_id, vec![bare_attempt(run_id)]);
        backend.save(&run).await.expect("SQL save should succeed");
    }

    /// Full round-trip: exercises every sub-result insert helper.
    #[tokio::test]
    #[ignore = "requires SQL Server -- set NETWORKER_SQL_CONN to enable"]
    async fn db_mssql_full_round_trip() {
        let base_conn = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let (_db_conn, backend) =
            isolated_backend_with_documented_schema(&base_conn, "full_round_trip", false).await;
        let run_id = Uuid::new_v4();
        let run = make_run(run_id, vec![bare_attempt(run_id), full_attempt(run_id)]);
        backend
            .save(&run)
            .await
            .expect("SQL full save should succeed");
    }

    /// Insert a TestRun then SELECT it back and verify every column.
    #[tokio::test]
    #[ignore = "requires SQL Server -- set NETWORKER_SQL_CONN to enable"]
    async fn db_mssql_verify_test_run_fields() {
        let base_conn = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let (conn_str, backend) =
            isolated_backend_with_documented_schema(&base_conn, "verify_run_fields", false).await;
        let run_id = Uuid::new_v4();
        let attempt = bare_attempt(run_id);
        let run = make_run(run_id, vec![attempt]);
        backend.save(&run).await.unwrap();

        let mut client = test_connect(&conn_str).await;
        let sql = format!(
            "SELECT RunId, TargetUrl, TargetHost, Modes, TotalRuns, \
             Concurrency, TimeoutMs, ClientOs, ClientVersion, \
             SuccessCount, FailureCount \
             FROM dbo.TestRun WHERE RunId = '{}'",
            run_id
        );
        let row = query_one(&mut client, &sql)
            .await
            .expect("TestRun row must exist");

        let db_run_id: &str = row.get(0).unwrap();
        assert_eq!(db_run_id, run_id.to_string());
        let db_url: &str = row.get(1).unwrap();
        assert_eq!(db_url, "http://localhost/health");
        let db_host: &str = row.get(2).unwrap();
        assert_eq!(db_host, "localhost");
        let db_modes: &str = row.get(3).unwrap();
        assert_eq!(db_modes, "http1");
        let db_total: i32 = row.get(4).unwrap();
        assert_eq!(db_total, 1);
        let db_conc: i32 = row.get(5).unwrap();
        assert_eq!(db_conc, 1);
        let db_timeout: i64 = row.get(6).unwrap();
        assert_eq!(db_timeout, 5000);
        let db_os: &str = row.get(7).unwrap();
        assert_eq!(db_os, std::env::consts::OS);
        let db_version: &str = row.get(8).unwrap();
        assert_eq!(db_version, env!("CARGO_PKG_VERSION"));
        let db_success: i32 = row.get(9).unwrap();
        assert_eq!(db_success, 1);
        let db_fail: i32 = row.get(10).unwrap();
        assert_eq!(db_fail, 0);
    }

    /// Insert a full attempt then SELECT back each sub-result table row.
    #[tokio::test]
    #[ignore = "requires SQL Server -- set NETWORKER_SQL_CONN to enable"]
    async fn db_mssql_verify_all_sub_results() {
        let base_conn = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let (conn_str, backend) =
            isolated_backend_with_documented_schema(&base_conn, "verify_sub_results", false).await;
        let run_id = Uuid::new_v4();
        let attempt = full_attempt(run_id);
        let attempt_id = attempt.attempt_id;
        let run = make_run(run_id, vec![attempt]);
        backend.save(&run).await.unwrap();

        let mut c = test_connect(&conn_str).await;
        let aid = attempt_id.to_string();

        // RequestAttempt
        let row = query_one(
            &mut c,
            &format!(
                "SELECT Protocol, SequenceNum, Success, RetryCount \
                 FROM dbo.RequestAttempt WHERE AttemptId = '{aid}'"
            ),
        )
        .await
        .expect("RequestAttempt row");
        let proto: &str = row.get(0).unwrap();
        assert_eq!(proto, "http1");
        let seq: i32 = row.get(1).unwrap();
        assert_eq!(seq, 1);
        let success: bool = row.get(2).unwrap();
        assert!(!success);
        let retry: i32 = row.get(3).unwrap();
        assert_eq!(retry, 2);

        // DnsResult
        let row = query_one(
            &mut c,
            &format!(
                "SELECT QueryName, ResolvedIPs, DurationMs, Success \
                 FROM dbo.DnsResult WHERE AttemptId = '{aid}'"
            ),
        )
        .await
        .expect("DnsResult row");
        let qname: &str = row.get(0).unwrap();
        assert_eq!(qname, "localhost");
        let ips: &str = row.get(1).unwrap();
        assert_eq!(ips, "127.0.0.1");
        let dur: f64 = row.get(2).unwrap();
        assert!((dur - 1.5).abs() < 0.01);
        let dns_ok: bool = row.get(3).unwrap();
        assert!(dns_ok);

        // TcpResult
        let row = query_one(
            &mut c,
            &format!(
                "SELECT RemoteAddr, ConnectDurationMs, MssBytesEstimate, \
                 RttEstimateMs, CongestionAlgorithm, DeliveryRateBps, MinRttMs \
                 FROM dbo.TcpResult WHERE AttemptId = '{aid}'"
            ),
        )
        .await
        .expect("TcpResult row");
        let remote: &str = row.get(0).unwrap();
        assert_eq!(remote, "127.0.0.1:8080");
        let connect_ms: f64 = row.get(1).unwrap();
        assert!((connect_ms - 0.5).abs() < 0.01);
        let mss: Option<i32> = row.get(2);
        assert_eq!(mss, Some(1460));
        let rtt: Option<f64> = row.get(3);
        assert!((rtt.unwrap() - 0.3).abs() < 0.01);
        let algo: Option<&str> = row.get(4);
        assert_eq!(algo, Some("cubic"));
        let delivery: Option<i64> = row.get(5);
        assert_eq!(delivery, Some(1_000_000));
        let min_rtt: Option<f64> = row.get(6);
        assert!((min_rtt.unwrap() - 0.2).abs() < 0.01);

        // TlsResult
        let row = query_one(
            &mut c,
            &format!(
                "SELECT ProtocolVersion, CipherSuite, AlpnNegotiated, \
                 CertSubject, HandshakeDurationMs \
                 FROM dbo.TlsResult WHERE AttemptId = '{aid}'"
            ),
        )
        .await
        .expect("TlsResult row");
        let ver: &str = row.get(0).unwrap();
        assert_eq!(ver, "TLSv1.3");
        let cipher: &str = row.get(1).unwrap();
        assert_eq!(cipher, "TLS_AES_256_GCM_SHA384");
        let alpn: Option<&str> = row.get(2);
        assert_eq!(alpn, Some("http/1.1"));
        let subj: Option<&str> = row.get(3);
        assert_eq!(subj, Some("CN=localhost"));
        let hs_ms: f64 = row.get(4).unwrap();
        assert!((hs_ms - 5.0).abs() < 0.01);

        // HttpResult
        let row = query_one(
            &mut c,
            &format!(
                "SELECT NegotiatedVersion, StatusCode, TtfbMs, TotalDurationMs, \
                 PayloadBytes, ThroughputMbps \
                 FROM dbo.HttpResult WHERE AttemptId = '{aid}'"
            ),
        )
        .await
        .expect("HttpResult row");
        let http_ver: &str = row.get(0).unwrap();
        assert_eq!(http_ver, "HTTP/1.1");
        let status: i32 = row.get(1).unwrap();
        assert_eq!(status, 200);
        let ttfb: f64 = row.get(2).unwrap();
        assert!((ttfb - 8.0).abs() < 0.01);
        let total: f64 = row.get(3).unwrap();
        assert!((total - 12.0).abs() < 0.01);
        let payload: Option<i64> = row.get(4);
        assert_eq!(payload, Some(65536));
        let tput: Option<f64> = row.get(5);
        assert!((tput.unwrap() - 105.0).abs() < 0.01);

        // UdpResult
        let row = query_one(
            &mut c,
            &format!(
                "SELECT ProbeCount, SuccessCount, LossPercent, \
                 RttMinMs, RttAvgMs, RttP95Ms, JitterMs \
                 FROM dbo.UdpResult WHERE AttemptId = '{aid}'"
            ),
        )
        .await
        .expect("UdpResult row");
        let probes: i32 = row.get(0).unwrap();
        assert_eq!(probes, 5);
        let successes: i32 = row.get(1).unwrap();
        assert_eq!(successes, 4);
        let loss: f64 = row.get(2).unwrap();
        assert!((loss - 20.0).abs() < 0.01);
        let rtt_min_val: f64 = row.get(3).unwrap();
        assert!((rtt_min_val - 0.1).abs() < 0.01);
        let rtt_avg: f64 = row.get(4).unwrap();
        assert!((rtt_avg - 0.25).abs() < 0.01);
        let rtt_p95: f64 = row.get(5).unwrap();
        assert!((rtt_p95 - 0.4).abs() < 0.01);
        let jitter: f64 = row.get(6).unwrap();
        assert!((jitter - 0.05).abs() < 0.01);

        // ErrorRecord
        let row = query_one(
            &mut c,
            &format!(
                "SELECT ErrorCategory, ErrorMessage, ErrorDetail \
                 FROM dbo.ErrorRecord WHERE AttemptId = '{aid}'"
            ),
        )
        .await
        .expect("ErrorRecord row");
        let cat: &str = row.get(0).unwrap();
        assert_eq!(cat, "http");
        let msg: &str = row.get(1).unwrap();
        assert_eq!(msg, "simulated error");
        let detail: Option<&str> = row.get(2);
        assert_eq!(detail, Some("detail text"));

        // ServerTimingResult
        let row = query_one(
            &mut c,
            &format!(
                "SELECT RequestId, ClockSkewMs, ProcessingMs, TotalServerMs \
                 FROM dbo.ServerTimingResult WHERE AttemptId = '{aid}'"
            ),
        )
        .await
        .expect("ServerTimingResult row");
        let req_id: Option<&str> = row.get(0);
        assert_eq!(req_id, Some("req-abc-123"));
        let skew: Option<f64> = row.get(1);
        assert!((skew.unwrap() - 0.5).abs() < 0.01);
        let proc_ms: Option<f64> = row.get(2);
        assert!((proc_ms.unwrap() - 3.0).abs() < 0.01);
        let total_srv: Option<f64> = row.get(3);
        assert!((total_srv.unwrap() - 4.0).abs() < 0.01);
    }

    /// Verify CASCADE DELETE: deleting a TestRun removes all child rows.
    #[tokio::test]
    #[ignore = "requires SQL Server -- set NETWORKER_SQL_CONN to enable"]
    async fn db_mssql_cascade_delete() {
        let base_conn = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let (conn_str, backend) =
            isolated_backend_with_documented_schema(&base_conn, "cascade_delete", false).await;
        let run_id = Uuid::new_v4();
        let attempt = full_attempt(run_id);
        let attempt_id = attempt.attempt_id;
        let run = make_run(run_id, vec![attempt]);
        backend.save(&run).await.unwrap();

        let mut c = test_connect(&conn_str).await;
        let rid = run_id.to_string();
        let aid = attempt_id.to_string();

        let rows = query_all(
            &mut c,
            &format!("SELECT 1 FROM dbo.RequestAttempt WHERE RunId = '{rid}'"),
        )
        .await;
        assert!(!rows.is_empty(), "attempt should exist before delete");

        // ErrorRecord and ServerTimingResult have FKs with ON DELETE NO ACTION.
        for table in &["ErrorRecord", "ServerTimingResult"] {
            c.execute(
                &format!("DELETE FROM dbo.{table} WHERE AttemptId = '{aid}'") as &str,
                &[],
            )
            .await
            .unwrap();
        }

        c.execute(
            &format!("DELETE FROM dbo.TestRun WHERE RunId = '{rid}'") as &str,
            &[],
        )
        .await
        .unwrap();

        let rows = query_all(
            &mut c,
            &format!("SELECT 1 FROM dbo.RequestAttempt WHERE RunId = '{rid}'"),
        )
        .await;
        assert!(rows.is_empty(), "attempts should be cascade-deleted");

        let rows = query_all(
            &mut c,
            &format!("SELECT 1 FROM dbo.DnsResult WHERE AttemptId = '{aid}'"),
        )
        .await;
        assert!(rows.is_empty(), "DNS results should be cascade-deleted");

        let rows = query_all(
            &mut c,
            &format!("SELECT 1 FROM dbo.HttpResult WHERE AttemptId = '{aid}'"),
        )
        .await;
        assert!(rows.is_empty(), "HTTP results should be cascade-deleted");
    }

    /// Verify duplicate RunId insertion fails (PK constraint).
    #[tokio::test]
    #[ignore = "requires SQL Server -- set NETWORKER_SQL_CONN to enable"]
    async fn db_mssql_duplicate_run_id_fails() {
        let base_conn = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let (conn_str, backend) =
            isolated_backend_with_documented_schema(&base_conn, "duplicate_run", false).await;
        let run_id = Uuid::new_v4();
        let run = make_run(run_id, vec![bare_attempt(run_id)]);
        backend.save(&run).await.unwrap();

        // Second insert with same RunId should fail on PK.
        let run2 = make_run(run_id, vec![bare_attempt(run_id)]);
        let backend2 = MssqlBackend::connect(&conn_str).await.unwrap();
        let err = backend2.save(&run2).await;
        assert!(err.is_err(), "duplicate RunId should fail");
    }

    /// Insert multiple attempts in one run, verify correct count.
    #[tokio::test]
    #[ignore = "requires SQL Server -- set NETWORKER_SQL_CONN to enable"]
    async fn db_mssql_multiple_attempts_count() {
        let base_conn = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let (conn_str, backend) =
            isolated_backend_with_documented_schema(&base_conn, "multiple_attempts", false).await;
        let run_id = Uuid::new_v4();
        let attempts = vec![
            bare_attempt(run_id),
            bare_attempt(run_id),
            full_attempt(run_id),
        ];
        let mut run = make_run(run_id, attempts);
        run.total_runs = 3;
        backend.save(&run).await.unwrap();

        let mut c = test_connect(&conn_str).await;
        let rid = run_id.to_string();

        let rows = query_all(
            &mut c,
            &format!("SELECT 1 FROM dbo.RequestAttempt WHERE RunId = '{rid}'"),
        )
        .await;
        assert_eq!(rows.len(), 3);

        let dns_rows = query_all(
            &mut c,
            &format!(
                "SELECT 1 FROM dbo.DnsResult d \
                 JOIN dbo.RequestAttempt a ON d.AttemptId = a.AttemptId \
                 WHERE a.RunId = '{rid}'"
            ),
        )
        .await;
        assert_eq!(dns_rows.len(), 1);

        let http_rows = query_all(
            &mut c,
            &format!(
                "SELECT 1 FROM dbo.HttpResult h \
                 JOIN dbo.RequestAttempt a ON h.AttemptId = a.AttemptId \
                 WHERE a.RunId = '{rid}'"
            ),
        )
        .await;
        assert_eq!(http_rows.len(), 1);
    }

    /// Verify bare attempt leaves no orphan rows in child tables.
    #[tokio::test]
    #[ignore = "requires SQL Server -- set NETWORKER_SQL_CONN to enable"]
    async fn db_mssql_bare_attempt_no_child_rows() {
        let base_conn = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let (conn_str, backend) =
            isolated_backend_with_documented_schema(&base_conn, "bare_attempt", false).await;
        let run_id = Uuid::new_v4();
        let attempt = bare_attempt(run_id);
        let aid = attempt.attempt_id.to_string();
        let run = make_run(run_id, vec![attempt]);
        backend.save(&run).await.unwrap();

        let mut c = test_connect(&conn_str).await;
        for table in &[
            "DnsResult",
            "TcpResult",
            "TlsResult",
            "HttpResult",
            "UdpResult",
            "ErrorRecord",
            "ServerTimingResult",
        ] {
            let rows = query_all(
                &mut c,
                &format!("SELECT 1 FROM dbo.{table} WHERE AttemptId = '{aid}'"),
            )
            .await;
            assert!(rows.is_empty(), "bare attempt should have no {table} rows");
        }
    }

    #[tokio::test]
    #[ignore = "requires SQL Server -- set NETWORKER_SQL_CONN and apply sql/09_BenchmarkSchema.sql"]
    async fn db_mssql_persists_benchmark_rows() {
        let base_conn = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let (conn_str, backend) =
            isolated_backend_with_documented_schema(&base_conn, "persist_benchmark", true).await;
        let run_id = Uuid::new_v4();
        let run = make_benchmark_run(run_id, vec![bare_attempt(run_id)]);
        backend.save(&run).await.unwrap();

        let mut c = test_connect(&conn_str).await;
        if !benchmark_schema_installed(&mut c).await.unwrap() {
            return;
        }

        let rid = run_id.to_string();
        let row = query_one(
            &mut c,
            &format!(
                "SELECT ContractVersion, TargetHost \
                 FROM dbo.BenchmarkRun WHERE BenchmarkRunId = '{rid}'"
            ),
        )
        .await
        .expect("BenchmarkRun row must exist");
        let contract_version: &str = row.get(0).unwrap();
        assert_eq!(contract_version, "1.2");
        let target_host: &str = row.get(1).unwrap();
        assert_eq!(target_host, "localhost");

        let row = query_one(
            &mut c,
            &format!(
                "SELECT LaunchIndex, Scenario, PrimaryPhase, WarmupSampleCount \
                 FROM dbo.BenchmarkLaunch WHERE BenchmarkRunId = '{rid}'"
            ),
        )
        .await
        .expect("BenchmarkLaunch row");
        let launch_index: i32 = row.get(0).unwrap();
        assert_eq!(launch_index, 0);
        let scenario: &str = row.get(1).unwrap();
        assert_eq!(scenario, "warm");
        let primary_phase: &str = row.get(2).unwrap();
        assert_eq!(primary_phase, "measured");
        let warmup_sample_count: i64 = row.get(3).unwrap();
        assert_eq!(warmup_sample_count, 0);

        let row = query_one(
            &mut c,
            &format!(
                "SELECT CaseId, Protocol, MetricUnit \
                 FROM dbo.BenchmarkCase WHERE BenchmarkRunId = '{rid}'"
            ),
        )
        .await
        .expect("BenchmarkCase row");
        let case_id: &str = row.get(0).unwrap();
        assert_eq!(case_id, "http1:default:default");
        let protocol: &str = row.get(1).unwrap();
        assert_eq!(protocol, "http1");
        let metric_unit: &str = row.get(2).unwrap();
        assert_eq!(metric_unit, "ms");

        let row = query_one(
            &mut c,
            &format!(
                "SELECT InclusionStatus, MetricUnit \
                 FROM dbo.BenchmarkSample WHERE BenchmarkRunId = '{rid}'"
            ),
        )
        .await
        .expect("BenchmarkSample row");
        let inclusion_status: &str = row.get(0).unwrap();
        assert_eq!(inclusion_status, "excluded_missing_metric");
        let sample_metric_unit: &str = row.get(1).unwrap();
        assert_eq!(sample_metric_unit, "ms");

        let row = query_one(
            &mut c,
            &format!(
                "SELECT SampleCount, IncludedSampleCount, FailureCount \
                 FROM dbo.BenchmarkSummary \
                 WHERE BenchmarkRunId = '{rid}' AND CaseId = 'http1:default:default'"
            ),
        )
        .await
        .expect("BenchmarkSummary row");
        let sample_count: i64 = row.get(0).unwrap();
        assert_eq!(sample_count, 1);
        let included_sample_count: i64 = row.get(1).unwrap();
        assert_eq!(included_sample_count, 0);
        let failure_count: i64 = row.get(2).unwrap();
        assert_eq!(failure_count, 0);
    }

    #[tokio::test]
    #[ignore = "requires SQL Server -- set NETWORKER_SQL_CONN to enable"]
    async fn db_mssql_plain_run_succeeds_with_documented_base_schema() {
        let base_conn = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let (db_conn, backend) =
            isolated_backend_with_documented_schema(&base_conn, "base_plain", false).await;
        let run_id = Uuid::new_v4();
        let run = make_run(run_id, vec![bare_attempt(run_id)]);
        backend.save(&run).await.unwrap();

        let mut client = test_connect(&db_conn).await;
        let test_run_row = query_one(
            &mut client,
            &format!("SELECT COUNT_BIG(*) FROM dbo.TestRun WHERE RunId = '{run_id}'"),
        )
        .await
        .expect("TestRun count row");
        let test_run_count: i64 = test_run_row.get(0).unwrap();
        assert_eq!(test_run_count, 1);

        let attempt_row = query_one(
            &mut client,
            &format!("SELECT COUNT_BIG(*) FROM dbo.RequestAttempt WHERE RunId = '{run_id}'"),
        )
        .await
        .expect("RequestAttempt count row");
        let attempt_count: i64 = attempt_row.get(0).unwrap();
        assert_eq!(attempt_count, 1);

        assert!(!benchmark_table_exists(&mut client, "BenchmarkRun").await);
    }

    #[tokio::test]
    #[ignore = "requires SQL Server -- set NETWORKER_SQL_CONN to enable"]
    async fn db_mssql_benchmark_run_skips_benchmark_rows_with_documented_base_schema() {
        let base_conn = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let (db_conn, backend) =
            isolated_backend_with_documented_schema(&base_conn, "base_bench", false).await;
        let run_id = Uuid::new_v4();
        let run = make_benchmark_run(run_id, vec![bare_attempt(run_id)]);
        backend.save(&run).await.unwrap();

        let mut client = test_connect(&db_conn).await;
        let test_run_row = query_one(
            &mut client,
            &format!("SELECT COUNT_BIG(*) FROM dbo.TestRun WHERE RunId = '{run_id}'"),
        )
        .await
        .expect("TestRun count row");
        let test_run_count: i64 = test_run_row.get(0).unwrap();
        assert_eq!(test_run_count, 1);

        assert!(!benchmark_table_exists(&mut client, "BenchmarkRun").await);
    }

    #[tokio::test]
    #[ignore = "requires SQL Server -- set NETWORKER_SQL_CONN to enable"]
    async fn db_mssql_plain_run_does_not_create_benchmark_rows_when_schema_exists() {
        let base_conn = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let (db_conn, backend) =
            isolated_backend_with_documented_schema(&base_conn, "migrated_plain", true).await;
        let run_id = Uuid::new_v4();
        let run = make_run(run_id, vec![bare_attempt(run_id)]);
        backend.save(&run).await.unwrap();

        let mut client = test_connect(&db_conn).await;
        let row = query_one(
            &mut client,
            &format!("SELECT COUNT_BIG(*) FROM dbo.BenchmarkRun WHERE BenchmarkRunId = '{run_id}'"),
        )
        .await
        .expect("BenchmarkRun count row");
        let benchmark_run_count: i64 = row.get(0).unwrap();
        assert_eq!(benchmark_run_count, 0);
    }

    #[tokio::test]
    #[ignore = "requires SQL Server -- set NETWORKER_SQL_CONN to enable"]
    async fn db_mssql_benchmark_run_persists_rows_with_documented_benchmark_schema() {
        let base_conn = match sql_conn() {
            Some(c) => c,
            None => return,
        };
        let (db_conn, backend) =
            isolated_backend_with_documented_schema(&base_conn, "migrated_bench", true).await;
        let run_id = Uuid::new_v4();
        let run = make_benchmark_run(run_id, vec![bare_attempt(run_id)]);
        backend.save(&run).await.unwrap();

        let mut c = test_connect(&db_conn).await;
        let rid = run_id.to_string();

        let row = query_one(
            &mut c,
            &format!(
                "SELECT ContractVersion, TargetHost \
                 FROM dbo.BenchmarkRun WHERE BenchmarkRunId = '{rid}'"
            ),
        )
        .await
        .expect("BenchmarkRun row must exist");
        let contract_version: &str = row.get(0).unwrap();
        assert_eq!(contract_version, "1.2");
        let target_host: &str = row.get(1).unwrap();
        assert_eq!(target_host, "localhost");

        let env_row = query_one(
            &mut c,
            &format!(
                "SELECT COUNT_BIG(*) FROM dbo.BenchmarkEnvironment WHERE BenchmarkRunId = '{rid}'"
            ),
        )
        .await
        .expect("BenchmarkEnvironment count row");
        let env_count: i64 = env_row.get(0).unwrap();
        assert_eq!(env_count, 1);

        let quality_row = query_one(
            &mut c,
            &format!(
                "SELECT COUNT_BIG(*) FROM dbo.BenchmarkDataQuality WHERE BenchmarkRunId = '{rid}'"
            ),
        )
        .await
        .expect("BenchmarkDataQuality count row");
        let quality_count: i64 = quality_row.get(0).unwrap();
        assert_eq!(quality_count, 1);

        let row = query_one(
            &mut c,
            &format!(
                "SELECT LaunchIndex, Scenario, PrimaryPhase, WarmupSampleCount \
                 FROM dbo.BenchmarkLaunch WHERE BenchmarkRunId = '{rid}'"
            ),
        )
        .await
        .expect("BenchmarkLaunch row");
        let launch_index: i32 = row.get(0).unwrap();
        assert_eq!(launch_index, 0);
        let scenario: &str = row.get(1).unwrap();
        assert_eq!(scenario, "warm");
        let primary_phase: &str = row.get(2).unwrap();
        assert_eq!(primary_phase, "measured");
        let warmup_sample_count: i64 = row.get(3).unwrap();
        assert_eq!(warmup_sample_count, 0);

        let row = query_one(
            &mut c,
            &format!(
                "SELECT CaseId, Protocol, MetricUnit \
                 FROM dbo.BenchmarkCase WHERE BenchmarkRunId = '{rid}'"
            ),
        )
        .await
        .expect("BenchmarkCase row");
        let case_id: &str = row.get(0).unwrap();
        assert_eq!(case_id, "http1:default:default");
        let protocol: &str = row.get(1).unwrap();
        assert_eq!(protocol, "http1");
        let metric_unit: &str = row.get(2).unwrap();
        assert_eq!(metric_unit, "ms");

        let row = query_one(
            &mut c,
            &format!(
                "SELECT InclusionStatus, MetricUnit \
                 FROM dbo.BenchmarkSample WHERE BenchmarkRunId = '{rid}'"
            ),
        )
        .await
        .expect("BenchmarkSample row");
        let inclusion_status: &str = row.get(0).unwrap();
        assert_eq!(inclusion_status, "excluded_missing_metric");
        let sample_metric_unit: &str = row.get(1).unwrap();
        assert_eq!(sample_metric_unit, "ms");

        let row = query_one(
            &mut c,
            &format!(
                "SELECT SampleCount, IncludedSampleCount, FailureCount \
                 FROM dbo.BenchmarkSummary \
                 WHERE BenchmarkRunId = '{rid}' AND CaseId = 'http1:default:default'"
            ),
        )
        .await
        .expect("BenchmarkSummary row");
        let sample_count: i64 = row.get(0).unwrap();
        assert_eq!(sample_count, 1);
        let included_sample_count: i64 = row.get(1).unwrap();
        assert_eq!(included_sample_count, 0);
        let failure_count: i64 = row.get(2).unwrap();
        assert_eq!(failure_count, 0);
    }
}
