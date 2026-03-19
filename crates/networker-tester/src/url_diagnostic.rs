use crate::metrics::{
    Protocol, UrlDiagnosticStatus, UrlPageLoadStrategy, UrlProbeAttemptType, UrlTestProtocolRun,
    UrlTestResource, UrlTestRun,
};
#[cfg(any(test, feature = "browser"))]
use crate::metrics::{UrlConnectionSummary, UrlOriginSummary};
use crate::runner::browser::find_chrome_binary;
use crate::runner::http::{run_probe, RunConfig as HttpRunConfig};
use crate::runner::http3::run_http3_probe;
use anyhow::Context;
use chrono::Utc;
use serde::{Deserialize, Serialize};
#[cfg(any(test, feature = "browser"))]
use std::collections::{BTreeMap, BTreeSet};
#[cfg(any(test, feature = "browser"))]
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Request contract for a URL page-load diagnostic run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlDiagnosticRequest {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookie: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default = "default_follow_redirects")]
    pub follow_redirects: bool,
    #[serde(default)]
    pub capture_pcap: bool,
    #[serde(default)]
    pub capture_har: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_force: Option<String>,
    #[serde(default = "default_http3_repeat_count")]
    pub http3_repeat_count: u32,
    #[serde(default)]
    pub ignore_tls_validation: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_engine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_idle_timeout_ms: Option<u64>,
}

fn default_follow_redirects() -> bool {
    true
}

fn default_http3_repeat_count() -> u32 {
    10
}

/// Runtime capability flags for optional subsystems.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UrlDiagnosticCapabilities {
    pub browser_available: bool,
    pub har_available: bool,
    pub pcap_available: bool,
    pub protocol_probe_available: bool,
}

/// Normalized run command ready for orchestration.
#[derive(Debug, Clone)]
pub struct UrlDiagnosticPlan {
    pub run: UrlTestRun,
    pub request: UrlDiagnosticRequest,
    pub capabilities: UrlDiagnosticCapabilities,
}

#[derive(Debug, Clone)]
pub struct UrlDiagnosticOrchestrator {
    capabilities: UrlDiagnosticCapabilities,
}

impl UrlDiagnosticOrchestrator {
    pub fn new(capabilities: UrlDiagnosticCapabilities) -> Self {
        Self { capabilities }
    }

    pub fn detect_capabilities() -> UrlDiagnosticCapabilities {
        UrlDiagnosticCapabilities {
            browser_available: find_chrome_binary().is_some(),
            har_available: cfg!(feature = "browser"),
            pcap_available: crate::capture::detect_tshark().is_some(),
            protocol_probe_available: true,
        }
    }

    pub fn capabilities(&self) -> UrlDiagnosticCapabilities {
        self.capabilities
    }

    pub fn plan(&self, request: UrlDiagnosticRequest) -> anyhow::Result<UrlDiagnosticPlan> {
        request
            .validate()
            .context("validate URL diagnostic request")?;

        let now = Utc::now();
        let mut capture_errors = Vec::new();
        if request.capture_har && !self.capabilities.har_available {
            capture_errors.push("har capture unavailable in current runtime".to_string());
        }
        if request.capture_pcap && !self.capabilities.pcap_available {
            capture_errors.push("pcap capture unavailable in current runtime".to_string());
        }
        if !self.capabilities.browser_available {
            capture_errors
                .push("browser diagnostic engine unavailable in current runtime".to_string());
        }

        let run = UrlTestRun {
            id: Uuid::new_v4(),
            started_at: now,
            completed_at: None,
            requested_url: request.url.clone(),
            final_url: None,
            status: UrlDiagnosticStatus::Pending,
            page_load_strategy: UrlPageLoadStrategy::Browser,
            browser_engine: request.browser_engine.clone(),
            browser_version: None,
            user_agent: request.user_agent.clone(),
            primary_origin: None,
            observed_protocol_primary_load: None,
            advertised_alt_svc: None,
            validated_http_versions: Vec::new(),
            tls_version: None,
            cipher_suite: None,
            alpn: None,
            dns_ms: None,
            connect_ms: None,
            handshake_ms: None,
            ttfb_ms: None,
            dom_content_loaded_ms: None,
            load_event_ms: None,
            network_idle_ms: None,
            capture_end_ms: None,
            total_requests: 0,
            total_transfer_bytes: 0,
            peak_concurrent_connections: None,
            redirect_count: 0,
            failure_count: 0,
            har_path: None,
            pcap_path: None,
            pcap_summary: None,
            capture_errors,
            environment_notes: None,
            origin_summaries: Vec::new(),
            connection_summary: None,
            resources: Vec::new(),
            protocol_runs: Vec::new(),
        };

        Ok(UrlDiagnosticPlan {
            run,
            request,
            capabilities: self.capabilities,
        })
    }

    pub fn mark_running(&self, run: &mut UrlTestRun) {
        run.status = UrlDiagnosticStatus::Running;
    }

    pub fn mark_failed(&self, run: &mut UrlTestRun, reason: impl Into<String>) {
        run.status = UrlDiagnosticStatus::Failed;
        run.completed_at = Some(Utc::now());
        run.capture_errors.push(reason.into());
    }

    pub fn mark_partial(&self, run: &mut UrlTestRun, note: impl Into<String>) {
        run.status = UrlDiagnosticStatus::Partial;
        run.completed_at = Some(Utc::now());
        run.capture_errors.push(note.into());
    }

    pub fn mark_completed(&self, run: &mut UrlTestRun) {
        run.status = if run.capture_errors.is_empty() {
            UrlDiagnosticStatus::Completed
        } else {
            UrlDiagnosticStatus::Partial
        };
        run.completed_at = Some(Utc::now());
    }

    pub fn add_resource(&self, run: &mut UrlTestRun, resource: UrlTestResource) {
        run.total_requests += 1;
        if resource.failed {
            run.failure_count += 1;
        }
        if let Some(size) = resource.transfer_size {
            run.total_transfer_bytes += size;
        }
        run.resources.push(resource);
    }

    pub fn add_protocol_run(&self, run: &mut UrlTestRun, probe: UrlTestProtocolRun) {
        if probe.succeeded
            && !run
                .validated_http_versions
                .iter()
                .any(|v| v == &probe.protocol_mode)
        {
            run.validated_http_versions
                .push(probe.protocol_mode.clone());
        }
        run.protocol_runs.push(probe);
    }

    pub fn make_protocol_probe(
        &self,
        run_id: Uuid,
        protocol_mode: impl Into<String>,
        run_number: u32,
        attempt_type: UrlProbeAttemptType,
    ) -> UrlTestProtocolRun {
        UrlTestProtocolRun {
            url_test_run_id: run_id,
            protocol_mode: protocol_mode.into(),
            run_number,
            attempt_type,
            observed_protocol: None,
            fallback_occurred: None,
            succeeded: false,
            status_code: None,
            ttfb_ms: None,
            total_ms: None,
            failure_reason: None,
            error: None,
        }
    }

    pub async fn execute_protocol_validation_probes(
        &self,
        run: &mut UrlTestRun,
        request: &UrlDiagnosticRequest,
    ) -> anyhow::Result<()> {
        let target = url::Url::parse(&request.url).context("parse URL for protocol probes")?;
        let base_cfg = HttpRunConfig {
            timeout_ms: request.timeout_ms.unwrap_or(30_000),
            insecure: request.ignore_tls_validation,
            ..Default::default()
        };

        let modes: Vec<(&str, u32)> = match request.protocol_force.as_deref() {
            Some("h1") => vec![("h1", 1)],
            Some("h2") => vec![("h2", 1)],
            Some("h3") => vec![("h3", request.http3_repeat_count)],
            _ => vec![("h1", 1), ("h2", 1), ("h3", request.http3_repeat_count)],
        };

        if !self.capabilities.protocol_probe_available {
            run.capture_errors
                .push("protocol validation probes unavailable in current runtime".into());
            return Ok(());
        }

        for (mode, repeat) in modes {
            for run_number in 1..=repeat {
                let attempt = match mode {
                    "h1" => {
                        run_probe(run.id, run_number, Protocol::Http1, &target, &base_cfg).await
                    }
                    "h2" => {
                        run_probe(run.id, run_number, Protocol::Http2, &target, &base_cfg).await
                    }
                    "h3" => {
                        run_http3_probe(
                            run.id,
                            run_number,
                            &target,
                            base_cfg.timeout_ms,
                            base_cfg.insecure,
                            base_cfg.ca_bundle.as_deref(),
                        )
                        .await
                    }
                    _ => unreachable!("validated by request parser"),
                };
                self.add_protocol_run(
                    run,
                    protocol_probe_from_attempt(run.id, mode, run_number, attempt),
                );
            }
        }

        Ok(())
    }

    pub async fn execute_primary_page_diagnostic(
        &self,
        plan: UrlDiagnosticPlan,
    ) -> anyhow::Result<UrlTestRun> {
        execute_primary_page_diagnostic_impl(self, plan).await
    }
}

#[cfg(any(test, feature = "browser"))]
fn summarize_origins_and_connections(
    resources: &[UrlTestResource],
) -> (Vec<UrlOriginSummary>, Option<UrlConnectionSummary>) {
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
        agg.total_transfer_bytes += resource.transfer_size.unwrap_or(0);
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
        if let Some(reused) = resource.reused_connection {
            if reused {
                reused_resource_count += 1;
                if resource.connection_id.is_some() {
                    reused_connection_count += 1;
                }
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
            UrlOriginSummary {
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
        Some(UrlConnectionSummary {
            total_connection_ids: connection_ids.len() as u32,
            reused_connection_count,
            reused_resource_count,
            resources_with_connection_id,
            peak_origin_request_count,
        })
    };

    (origin_summaries, connection_summary)
}

fn protocol_probe_from_attempt(
    run_id: Uuid,
    protocol_mode: &str,
    run_number: u32,
    attempt: crate::metrics::RequestAttempt,
) -> UrlTestProtocolRun {
    UrlTestProtocolRun {
        url_test_run_id: run_id,
        protocol_mode: protocol_mode.to_string(),
        run_number,
        attempt_type: UrlProbeAttemptType::Probe,
        observed_protocol: attempt
            .http
            .as_ref()
            .map(|h| h.negotiated_version.clone())
            .or_else(|| attempt.tls.as_ref().and_then(|t| t.alpn_negotiated.clone()))
            .or_else(|| Some(protocol_mode.to_string())),
        fallback_occurred: Some(false),
        succeeded: attempt.success,
        status_code: attempt
            .http
            .as_ref()
            .map(|h| i32::from(h.status_code) as u16),
        ttfb_ms: attempt.http.as_ref().map(|h| h.ttfb_ms),
        total_ms: attempt
            .http
            .as_ref()
            .map(|h| h.total_duration_ms)
            .or_else(|| attempt.tls.as_ref().map(|t| t.handshake_duration_ms)),
        failure_reason: attempt.error.as_ref().map(|e| e.message.clone()),
        error: attempt.error.as_ref().and_then(|e| e.detail.clone()),
    }
}

#[cfg(any(test, feature = "browser"))]
#[derive(Debug, Serialize)]
struct HarLog {
    version: String,
    creator: HarCreator,
    pages: Vec<HarPage>,
    entries: Vec<HarEntry>,
}

#[cfg(any(test, feature = "browser"))]
#[derive(Debug, Serialize)]
struct HarCreator {
    name: String,
    version: String,
}

#[cfg(any(test, feature = "browser"))]
#[derive(Debug, Serialize)]
struct HarPage {
    started_date_time: chrono::DateTime<Utc>,
    id: String,
    title: String,
    page_timings: HarPageTimings,
}

#[cfg(any(test, feature = "browser"))]
#[derive(Debug, Serialize)]
struct HarPageTimings {
    on_content_load: f64,
    on_load: f64,
}

#[cfg(any(test, feature = "browser"))]
#[derive(Debug, Serialize)]
struct HarEntry {
    started_date_time: chrono::DateTime<Utc>,
    time: f64,
    request: HarRequest,
    response: HarResponse,
    cache: serde_json::Value,
    timings: HarTimings,
    pageref: String,
}

#[cfg(any(test, feature = "browser"))]
#[derive(Debug, Serialize)]
struct HarRequest {
    method: String,
    url: String,
    http_version: String,
    headers: Vec<HarHeader>,
    query_string: Vec<HarQueryParam>,
    headers_size: i64,
    body_size: i64,
}

#[cfg(any(test, feature = "browser"))]
#[derive(Debug, Serialize)]
struct HarResponse {
    status: i32,
    status_text: String,
    http_version: String,
    headers: Vec<HarHeader>,
    content: HarContent,
    redirect_url: String,
    headers_size: i64,
    body_size: i64,
}

#[cfg(any(test, feature = "browser"))]
#[derive(Debug, Serialize)]
struct HarContent {
    size: i64,
    mime_type: String,
}

#[cfg(any(test, feature = "browser"))]
#[derive(Debug, Serialize)]
struct HarHeader {
    name: String,
    value: String,
}

#[cfg(any(test, feature = "browser"))]
#[derive(Debug, Serialize)]
struct HarQueryParam {
    name: String,
    value: String,
}

#[cfg(any(test, feature = "browser"))]
#[derive(Debug, Serialize)]
struct HarTimings {
    blocked: f64,
    dns: f64,
    connect: f64,
    ssl: f64,
    send: f64,
    wait: f64,
    receive: f64,
}

#[cfg(any(test, feature = "browser"))]
fn write_har_artifact(run: &UrlTestRun, out_dir: &Path) -> anyhow::Result<Option<PathBuf>> {
    if run.resources.is_empty() {
        return Ok(None);
    }

    let page_id = format!("page-{}", run.id);
    let mut entries = Vec::new();
    for resource in &run.resources {
        let parsed = url::Url::parse(&resource.resource_url).ok();
        let query_string = parsed
            .as_ref()
            .map(|u| {
                u.query_pairs()
                    .map(|(k, v)| HarQueryParam {
                        name: k.to_string(),
                        value: v.to_string(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mime_type = resource
            .mime_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".into());
        let protocol = resource
            .protocol
            .clone()
            .unwrap_or_else(|| "unknown".into());
        entries.push(HarEntry {
            started_date_time: run.started_at,
            time: resource.duration_ms.unwrap_or(0.0),
            request: HarRequest {
                method: "GET".into(),
                url: resource.resource_url.clone(),
                http_version: protocol.clone(),
                headers: vec![],
                query_string,
                headers_size: -1,
                body_size: -1,
            },
            response: HarResponse {
                status: resource
                    .status_code
                    .unwrap_or(if resource.failed { 599 } else { 200 })
                    as i32,
                status_text: if resource.failed {
                    "FAILED".into()
                } else {
                    "OK".into()
                },
                http_version: protocol,
                headers: vec![],
                content: HarContent {
                    size: resource
                        .decoded_body_size
                        .unwrap_or(resource.transfer_size.unwrap_or(0))
                        as i64,
                    mime_type,
                },
                redirect_url: String::new(),
                headers_size: -1,
                body_size: resource.transfer_size.unwrap_or(0) as i64,
            },
            cache: serde_json::json!({}),
            timings: HarTimings {
                blocked: 0.0,
                dns: 0.0,
                connect: 0.0,
                ssl: 0.0,
                send: 0.0,
                wait: resource.duration_ms.unwrap_or(0.0),
                receive: 0.0,
            },
            pageref: page_id.clone(),
        });
    }

    let log = HarLog {
        version: "1.2".into(),
        creator: HarCreator {
            name: "networker-tester".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        pages: vec![HarPage {
            started_date_time: run.started_at,
            id: page_id,
            title: run
                .final_url
                .clone()
                .unwrap_or_else(|| run.requested_url.clone()),
            page_timings: HarPageTimings {
                on_content_load: run.dom_content_loaded_ms.unwrap_or(-1.0),
                on_load: run.load_event_ms.unwrap_or(-1.0),
            },
        }],
        entries,
    };

    std::fs::create_dir_all(out_dir)?;
    let har_path = out_dir.join(format!(
        "url-test-{}.har",
        run.started_at.format("%Y%m%d-%H%M%S")
    ));
    std::fs::write(
        &har_path,
        serde_json::to_string_pretty(&serde_json::json!({ "log": log }))?,
    )?;
    Ok(Some(har_path))
}

#[cfg(feature = "browser")]
async fn execute_primary_page_diagnostic_impl(
    orchestrator: &UrlDiagnosticOrchestrator,
    plan: UrlDiagnosticPlan,
) -> anyhow::Result<UrlTestRun> {
    use chromiumoxide::browser::{Browser, BrowserConfig};
    use chromiumoxide::cdp::browser_protocol::network::EventResponseReceived;
    use futures::StreamExt;
    use std::collections::HashMap;

    let mut run = plan.run;
    orchestrator.mark_running(&mut run);

    let chrome_path =
        find_chrome_binary().context("browser engine unavailable in current runtime")?;
    let browser_config = BrowserConfig::builder()
        .chrome_executable(chrome_path)
        .no_sandbox()
        .disable_gpu()
        .build()
        .context("build browser config")?;

    let (browser, mut handler) = Browser::launch(browser_config)
        .await
        .context("launch browser")?;
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let page = match browser.new_page("about:blank").await {
        Ok(page) => page,
        Err(e) => {
            handler_task.abort();
            return Err(anyhow::anyhow!("create browser page: {e}"));
        }
    };

    let mut response_events = page
        .event_listener::<EventResponseReceived>()
        .await
        .context("subscribe to network response events")?;

    let nav_result = tokio::time::timeout(
        std::time::Duration::from_millis(plan.request.timeout_ms.unwrap_or(30_000)),
        async {
            page.goto(&plan.request.url).await?;
            page.wait_for_navigation().await?;
            Ok::<_, anyhow::Error>(())
        },
    )
    .await;

    match nav_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let _ = browser.close().await;
            handler_task.abort();
            orchestrator.mark_failed(&mut run, format!("navigation failed: {e}"));
            return Ok(run);
        }
        Err(_) => {
            let _ = browser.close().await;
            handler_task.abort();
            orchestrator.mark_failed(&mut run, "navigation timed out");
            return Ok(run);
        }
    }

    let final_url_js = r#"window.location.href"#;
    if let Ok(v) = page.evaluate(final_url_js).await {
        if let Some(final_url) = v.into_value::<String>() {
            run.final_url = Some(final_url);
        }
    }

    let timing_js = r#"
        (function() {
            var nav = performance.getEntriesByType('navigation')[0];
            var resources = performance.getEntriesByType('resource');
            var originSet = new Set();
            var items = [];
            for (var i = 0; i < resources.length; i++) {
                var r = resources[i];
                try { originSet.add(new URL(r.name).origin); } catch (_) {}
                items.push({
                    resourceUrl: r.name,
                    origin: (() => { try { return new URL(r.name).origin; } catch (_) { return ''; } })(),
                    resourceType: r.initiatorType || 'other',
                    transferSize: Number(r.transferSize || 0),
                    encodedBodySize: Number(r.encodedBodySize || 0),
                    decodedBodySize: Number(r.decodedBodySize || 0),
                    durationMs: Number(r.duration || 0),
                    fromCache: Number(r.transferSize || 0) === 0 && Number(r.decodedBodySize || 0) > 0
                });
            }
            return JSON.stringify({
                finalUrl: window.location.href,
                primaryOrigin: window.location.origin,
                domContentLoadedMs: nav ? Number(nav.domContentLoadedEventEnd || 0) : 0,
                loadEventMs: nav ? Number(nav.loadEventEnd || 0) : 0,
                ttfbMs: nav ? Number(nav.responseStart || 0) : 0,
                dnsMs: nav ? Math.max(0, Number(nav.domainLookupEnd || 0) - Number(nav.domainLookupStart || 0)) : 0,
                connectMs: nav ? Math.max(0, Number(nav.connectEnd || 0) - Number(nav.connectStart || 0)) : 0,
                handshakeMs: nav ? Math.max(0, Number(nav.connectEnd || 0) - Number(nav.secureConnectionStart || 0)) : 0,
                resourceCount: items.length,
                origins: Array.from(originSet),
                resources: items
            });
        })()
    "#;

    let mut protocol_counts: HashMap<String, u32> = HashMap::new();
    let mut main_protocol = String::from("unknown");
    let mut first_resource = true;
    let mut resource_protocol_by_url: HashMap<String, String> = HashMap::new();

    let drain_deadline = tokio::time::sleep(std::time::Duration::from_millis(500));
    tokio::pin!(drain_deadline);
    loop {
        tokio::select! {
            event = response_events.next() => {
                match event {
                    Some(evt) => {
                        let proto = evt.response.protocol.as_deref().unwrap_or("unknown").to_lowercase();
                        let url = evt.response.url.clone();
                        if first_resource {
                            main_protocol = proto.clone();
                            first_resource = false;
                        }
                        *protocol_counts.entry(proto.clone()).or_insert(0) += 1;
                        resource_protocol_by_url.entry(url).or_insert(proto);
                    }
                    None => break,
                }
            }
            _ = &mut drain_deadline => break,
        }
    }

    let timing_json = page
        .evaluate(timing_js)
        .await
        .context("extract navigation/resource timing")?
        .into_value::<String>()
        .unwrap_or_default();

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct PerfResource {
        resource_url: String,
        origin: String,
        resource_type: String,
        transfer_size: u64,
        encoded_body_size: u64,
        decoded_body_size: u64,
        duration_ms: f64,
        from_cache: bool,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct PerfSnapshot {
        final_url: String,
        primary_origin: String,
        dom_content_loaded_ms: f64,
        load_event_ms: f64,
        ttfb_ms: f64,
        dns_ms: f64,
        connect_ms: f64,
        handshake_ms: f64,
        resource_count: usize,
        origins: Vec<String>,
        resources: Vec<PerfResource>,
    }

    let snapshot: PerfSnapshot =
        serde_json::from_str(&timing_json).context("parse browser timing snapshot")?;

    run.final_url = Some(snapshot.final_url);
    run.primary_origin = Some(snapshot.primary_origin.clone());
    run.observed_protocol_primary_load = Some(main_protocol.clone());
    run.dom_content_loaded_ms = Some(snapshot.dom_content_loaded_ms);
    run.load_event_ms = Some(snapshot.load_event_ms);
    run.capture_end_ms = Some(snapshot.load_event_ms);
    run.ttfb_ms = Some(snapshot.ttfb_ms);
    run.dns_ms = Some(snapshot.dns_ms);
    run.connect_ms = Some(snapshot.connect_ms);
    run.handshake_ms = Some(snapshot.handshake_ms);
    run.total_requests = snapshot.resource_count as u32;
    run.environment_notes = Some(format!("origins_contacted={}", snapshot.origins.len()));

    for resource in snapshot.resources {
        orchestrator.add_resource(
            &mut run,
            UrlTestResource {
                url_test_run_id: run.id,
                resource_url: resource.resource_url.clone(),
                origin: if resource.origin.is_empty() {
                    snapshot.primary_origin.clone()
                } else {
                    resource.origin
                },
                resource_type: resource.resource_type,
                mime_type: None,
                status_code: None,
                protocol: resource_protocol_by_url
                    .get(&resource.resource_url)
                    .cloned(),
                transfer_size: Some(resource.transfer_size),
                encoded_body_size: Some(resource.encoded_body_size),
                decoded_body_size: Some(resource.decoded_body_size),
                duration_ms: Some(resource.duration_ms),
                connection_id: None,
                reused_connection: None,
                initiator_type: None,
                from_cache: Some(resource.from_cache),
                redirected: None,
                failed: false,
            },
        );
    }

    let (origin_summaries, connection_summary) = summarize_origins_and_connections(&run.resources);
    run.origin_summaries = origin_summaries;
    run.connection_summary = connection_summary;

    run.browser_engine
        .get_or_insert_with(|| "chromium".to_string());
    if let Ok(ver) = browser.version().await {
        run.browser_version = Some(ver.product);
    }

    if plan.request.capture_har {
        match write_har_artifact(&run, Path::new("./output")) {
            Ok(Some(path)) => run.har_path = Some(path.display().to_string()),
            Ok(None) => run
                .capture_errors
                .push("har capture requested but no resources were available to export".into()),
            Err(e) => run.capture_errors.push(format!("har export failed: {e}")),
        }
    }

    let _ = browser.close().await;
    handler_task.abort();
    orchestrator.mark_completed(&mut run);
    Ok(run)
}

#[cfg(not(feature = "browser"))]
async fn execute_primary_page_diagnostic_impl(
    orchestrator: &UrlDiagnosticOrchestrator,
    plan: UrlDiagnosticPlan,
) -> anyhow::Result<UrlTestRun> {
    let mut run = plan.run;
    orchestrator.mark_failed(
        &mut run,
        "browser primary page diagnostic requires '--features browser' (recompile to enable)",
    );
    Ok(run)
}

impl UrlDiagnosticRequest {
    pub fn validate(&self) -> anyhow::Result<()> {
        let parsed = url::Url::parse(&self.url).context("invalid URL")?;
        match parsed.scheme() {
            "http" | "https" => {}
            other => anyhow::bail!("unsupported URL scheme '{other}' (expected http or https)"),
        }
        if self.timeout_ms == Some(0) {
            anyhow::bail!("timeout_ms must be greater than zero when provided");
        }
        if self.network_idle_timeout_ms == Some(0) {
            anyhow::bail!("network_idle_timeout_ms must be greater than zero when provided");
        }
        if self.http3_repeat_count == 0 {
            anyhow::bail!("http3_repeat_count must be at least 1");
        }
        if let Some(force) = &self.protocol_force {
            match force.as_str() {
                "auto" | "h1" | "h2" | "h3" => {}
                _ => anyhow::bail!("protocol_force must be one of: auto, h1, h2, h3"),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn valid_request() -> UrlDiagnosticRequest {
        UrlDiagnosticRequest {
            url: "https://example.com".into(),
            auth_token: None,
            cookie: None,
            headers: Vec::new(),
            timeout_ms: Some(30_000),
            follow_redirects: true,
            capture_pcap: false,
            capture_har: false,
            protocol_force: Some("auto".into()),
            http3_repeat_count: 10,
            ignore_tls_validation: false,
            user_agent: None,
            browser_engine: Some("chromium".into()),
            network_idle_timeout_ms: Some(2_000),
        }
    }

    #[test]
    fn request_validation_rejects_bad_scheme() {
        let mut req = valid_request();
        req.url = "ftp://example.com".into();
        let err = req.validate().unwrap_err().to_string();
        assert!(err.contains("unsupported URL scheme"));
    }

    #[test]
    fn request_validation_rejects_zero_http3_repeat_count() {
        let mut req = valid_request();
        req.http3_repeat_count = 0;
        let err = req.validate().unwrap_err().to_string();
        assert!(err.contains("http3_repeat_count"));
    }

    #[test]
    fn request_validation_rejects_invalid_protocol_force() {
        let mut req = valid_request();
        req.protocol_force = Some("h9".into());
        let err = req.validate().unwrap_err().to_string();
        assert!(err.contains("protocol_force"));
    }

    #[test]
    fn planning_records_capability_gaps_as_capture_errors() {
        let req = valid_request();
        let orch = UrlDiagnosticOrchestrator::new(UrlDiagnosticCapabilities {
            browser_available: false,
            har_available: false,
            pcap_available: false,
            protocol_probe_available: true,
        });
        let plan = orch
            .plan(UrlDiagnosticRequest {
                capture_har: true,
                capture_pcap: true,
                ..req
            })
            .unwrap();
        assert_eq!(plan.run.status, UrlDiagnosticStatus::Pending);
        assert_eq!(plan.run.capture_errors.len(), 3);
    }

    #[test]
    fn lifecycle_mark_completed_without_errors_is_completed() {
        let orch = UrlDiagnosticOrchestrator::new(UrlDiagnosticCapabilities {
            browser_available: true,
            har_available: true,
            pcap_available: true,
            protocol_probe_available: true,
        });
        let mut run = orch.plan(valid_request()).unwrap().run;
        orch.mark_running(&mut run);
        orch.mark_completed(&mut run);
        assert_eq!(run.status, UrlDiagnosticStatus::Completed);
        assert!(run.completed_at.is_some());
    }

    #[test]
    fn lifecycle_mark_completed_with_errors_becomes_partial() {
        let orch = UrlDiagnosticOrchestrator::new(UrlDiagnosticCapabilities::default());
        let mut run = orch.plan(valid_request()).unwrap().run;
        run.capture_errors.push("har unavailable".into());
        orch.mark_completed(&mut run);
        assert_eq!(run.status, UrlDiagnosticStatus::Partial);
    }

    #[test]
    fn add_resource_updates_aggregate_counters() {
        let orch = UrlDiagnosticOrchestrator::new(UrlDiagnosticCapabilities::default());
        let mut run = orch.plan(valid_request()).unwrap().run;
        let run_id = run.id;
        orch.add_resource(
            &mut run,
            UrlTestResource {
                url_test_run_id: run_id,
                resource_url: "https://example.com/app.js".into(),
                origin: "https://example.com".into(),
                resource_type: "script".into(),
                mime_type: None,
                status_code: Some(200),
                protocol: Some("h2".into()),
                transfer_size: Some(2048),
                encoded_body_size: None,
                decoded_body_size: None,
                duration_ms: Some(10.0),
                connection_id: None,
                reused_connection: None,
                initiator_type: None,
                from_cache: None,
                redirected: None,
                failed: false,
            },
        );
        assert_eq!(run.total_requests, 1);
        assert_eq!(run.total_transfer_bytes, 2048);
        assert_eq!(run.failure_count, 0);
    }

    #[test]
    fn add_protocol_run_tracks_validated_versions_on_success() {
        let orch = UrlDiagnosticOrchestrator::new(UrlDiagnosticCapabilities::default());
        let mut run = orch.plan(valid_request()).unwrap().run;
        let mut probe = orch.make_protocol_probe(run.id, "h3", 1, UrlProbeAttemptType::Probe);
        probe.succeeded = true;
        orch.add_protocol_run(&mut run, probe);
        assert_eq!(run.validated_http_versions, vec!["h3"]);
    }

    #[cfg(not(feature = "browser"))]
    #[tokio::test]
    async fn execute_primary_page_diagnostic_without_browser_feature_marks_failed() {
        let orch = UrlDiagnosticOrchestrator::new(UrlDiagnosticCapabilities::default());
        let plan = orch.plan(valid_request()).unwrap();
        let run = orch.execute_primary_page_diagnostic(plan).await.unwrap();
        assert_eq!(run.status, UrlDiagnosticStatus::Failed);
        assert!(run
            .capture_errors
            .iter()
            .any(|e| e.contains("requires '--features browser'")));
    }

    #[tokio::test]
    async fn protocol_validation_unavailable_records_capture_error() {
        let orch = UrlDiagnosticOrchestrator::new(UrlDiagnosticCapabilities {
            browser_available: true,
            har_available: false,
            pcap_available: false,
            protocol_probe_available: false,
        });
        let req = valid_request();
        let mut run = orch.plan(req.clone()).unwrap().run;
        orch.execute_protocol_validation_probes(&mut run, &req)
            .await
            .unwrap();
        assert!(run
            .capture_errors
            .iter()
            .any(|e| e.contains("protocol validation probes unavailable")));
    }

    #[test]
    fn protocol_probe_from_failed_attempt_captures_error() {
        let attempt = crate::metrics::RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
            protocol: Protocol::Http1,
            sequence_num: 1,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            success: false,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: None,
            error: Some(crate::metrics::ErrorRecord {
                category: crate::metrics::ErrorCategory::Other,
                message: "connection failed".into(),
                detail: Some("refused".into()),
                occurred_at: Utc::now(),
            }),
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: None,
        };
        let probe = protocol_probe_from_attempt(Uuid::new_v4(), "h1", 1, attempt);
        assert!(!probe.succeeded);
        assert_eq!(probe.failure_reason.as_deref(), Some("connection failed"));
        assert_eq!(probe.error.as_deref(), Some("refused"));
    }

    #[test]
    fn summarize_origins_and_connections_groups_resources() {
        let run_id = Uuid::new_v4();
        let resources = vec![
            UrlTestResource {
                url_test_run_id: run_id,
                resource_url: "https://a.test/app.js".into(),
                origin: "https://a.test".into(),
                resource_type: "script".into(),
                mime_type: None,
                status_code: Some(200),
                protocol: Some("h3".into()),
                transfer_size: Some(100),
                encoded_body_size: None,
                decoded_body_size: None,
                duration_ms: Some(10.0),
                connection_id: Some("c1".into()),
                reused_connection: Some(false),
                initiator_type: None,
                from_cache: Some(false),
                redirected: Some(false),
                failed: false,
            },
            UrlTestResource {
                url_test_run_id: run_id,
                resource_url: "https://a.test/style.css".into(),
                origin: "https://a.test".into(),
                resource_type: "stylesheet".into(),
                mime_type: None,
                status_code: Some(200),
                protocol: Some("h3".into()),
                transfer_size: Some(50),
                encoded_body_size: None,
                decoded_body_size: None,
                duration_ms: Some(20.0),
                connection_id: Some("c1".into()),
                reused_connection: Some(true),
                initiator_type: None,
                from_cache: Some(true),
                redirected: Some(false),
                failed: false,
            },
            UrlTestResource {
                url_test_run_id: run_id,
                resource_url: "https://b.test/img.png".into(),
                origin: "https://b.test".into(),
                resource_type: "image".into(),
                mime_type: None,
                status_code: Some(404),
                protocol: Some("h2".into()),
                transfer_size: Some(25),
                encoded_body_size: None,
                decoded_body_size: None,
                duration_ms: Some(30.0),
                connection_id: Some("c2".into()),
                reused_connection: Some(false),
                initiator_type: None,
                from_cache: Some(false),
                redirected: Some(false),
                failed: true,
            },
        ];

        let (origins, conn) = summarize_origins_and_connections(&resources);
        assert_eq!(origins.len(), 2);
        assert_eq!(origins[0].origin, "https://a.test");
        assert_eq!(origins[0].request_count, 2);
        assert_eq!(origins[0].dominant_protocol.as_deref(), Some("h3"));
        assert_eq!(origins[0].cache_hit_count, Some(1));
        let conn = conn.expect("connection summary");
        assert_eq!(conn.total_connection_ids, 2);
        assert_eq!(conn.reused_resource_count, 1);
        assert_eq!(conn.peak_origin_request_count, Some(2));
    }

    #[test]
    fn detect_capabilities_reflects_tshark_presence() {
        let caps = UrlDiagnosticOrchestrator::detect_capabilities();
        assert_eq!(
            caps.pcap_available,
            crate::capture::detect_tshark().is_some()
        );
    }

    #[test]
    fn write_har_artifact_creates_har_file() {
        let dir = tempdir().unwrap();
        let run_id = Uuid::new_v4();
        let run = UrlTestRun {
            id: run_id,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            requested_url: "https://example.com".into(),
            final_url: Some("https://www.example.com".into()),
            status: UrlDiagnosticStatus::Completed,
            page_load_strategy: UrlPageLoadStrategy::Browser,
            browser_engine: Some("chromium".into()),
            browser_version: Some("123.0".into()),
            user_agent: None,
            primary_origin: Some("https://www.example.com".into()),
            observed_protocol_primary_load: Some("h3".into()),
            advertised_alt_svc: None,
            validated_http_versions: vec![],
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
            total_requests: 1,
            total_transfer_bytes: 2048,
            peak_concurrent_connections: None,
            redirect_count: 0,
            failure_count: 0,
            har_path: None,
            pcap_path: None,
            pcap_summary: None,
            capture_errors: vec![],
            environment_notes: None,
            origin_summaries: Vec::new(),
            connection_summary: None,
            resources: vec![UrlTestResource {
                url_test_run_id: run_id,
                resource_url: "https://www.example.com/app.js?v=1".into(),
                origin: "https://www.example.com".into(),
                resource_type: "script".into(),
                mime_type: Some("application/javascript".into()),
                status_code: Some(200),
                protocol: Some("h3".into()),
                transfer_size: Some(2048),
                encoded_body_size: Some(1800),
                decoded_body_size: Some(4096),
                duration_ms: Some(12.0),
                connection_id: None,
                reused_connection: None,
                initiator_type: Some("parser".into()),
                from_cache: Some(false),
                redirected: Some(false),
                failed: false,
            }],
            protocol_runs: vec![],
        };

        let path = write_har_artifact(&run, dir.path()).unwrap().unwrap();
        let contents = std::fs::read_to_string(path).unwrap();
        assert!(contents.contains("\"log\""));
        assert!(contents.contains("\"entries\""));
        assert!(contents.contains("app.js"));
    }
}
