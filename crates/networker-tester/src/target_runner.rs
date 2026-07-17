//! Per-target probe runner: warmup, benchmark phases (overhead / pilot /
//! measured / cooldown), the dispatch loop, and HTTP stack comparison probes.

use super::*;

// ─────────────────────────────────────────────────────────────────────────────
// Per-target probe runner
// ─────────────────────────────────────────────────────────────────────────────

pub(crate) async fn run_for_target(
    target_url_str: &str,
    cfg: &cli::ResolvedConfig,
    modes: &[Protocol],
    payload_sizes: &[usize],
    progress_reporter: Option<std::sync::Arc<ProgressReporter>>,
) -> anyhow::Result<TestRun> {
    let target = url::Url::parse(target_url_str)
        .with_context(|| format!("Invalid --target URL: {target_url_str}"))?;
    let target_host = target.host_str().unwrap_or("unknown").to_string();

    // ── ALPN warning for H2/H3/pageload2/browser over plain HTTP ─────────────
    for mode in modes {
        if matches!(
            mode,
            Protocol::Http2
                | Protocol::Http3
                | Protocol::PageLoad2
                | Protocol::PageLoad3
                | Protocol::Browser
                | Protocol::Browser2
                | Protocol::Browser3
        ) && target.scheme() == "http"
        {
            warn!(
                "{mode} requires HTTPS for ALPN negotiation; \
                 over plain http:// the connection falls back to HTTP/1.1. \
                 Use https:// (e.g. https://host:8443) with --insecure for the \
                 self-signed endpoint cert."
            );
        }
    }

    // ── Fetch server info (connectivity check + metadata) ─────────────────────
    let server_info = match fetch_server_info(&target, cfg.insecure).await {
        Some(info) => {
            info!(
                "Server: {} {} | {} cores | {} MB RAM | {} | v{} | region: {}",
                info.os,
                info.arch,
                info.cpu_cores,
                info.total_memory_mb.unwrap_or(0),
                info.os_version.as_deref().unwrap_or("?"),
                info.server_version.as_deref().unwrap_or("?"),
                info.region.as_deref().unwrap_or("unknown"),
            );
            Some(info)
        }
        None => {
            warn!(
                "Could not fetch server info from /info — server may not be a networker-endpoint"
            );
            None
        }
    };
    let client_info = Some(HostInfo::collect_local());

    let benchmark_environment_check = if cfg.benchmark_mode && cfg.benchmark_phase == "measured" {
        let samples = cfg
            .benchmark_environment_check_samples
            .unwrap_or(DEFAULT_ENVIRONMENT_CHECK_SAMPLES);
        let interval_ms = cfg
            .benchmark_environment_check_interval_ms
            .unwrap_or(DEFAULT_ENVIRONMENT_CHECK_INTERVAL_MS);
        let environment_check = measure_environment_check(&target, samples, interval_ms).await;
        match environment_check.as_ref() {
            Some(check) if check.successful_samples > 0 => {
                info!(
                    "Environment-check: {} | RTT avg={:.2}ms min={:.2}ms max={:.2}ms p50={:.2}ms p95={:.2}ms loss={:.1}% ({} / {} samples)",
                    check.network_type,
                    check.rtt_avg_ms,
                    check.rtt_min_ms,
                    check.rtt_max_ms,
                    check.rtt_p50_ms,
                    check.rtt_p95_ms,
                    check.packet_loss_percent,
                    check.successful_samples,
                    check.attempted_samples,
                );
            }
            Some(check) => {
                warn!(
                    "Environment-check: {} | RTT probes failed (loss={:.1}% across {} attempts)",
                    check.network_type, check.packet_loss_percent, check.attempted_samples,
                );
            }
            None => warn!("Could not perform benchmark environment-check"),
        }
        environment_check
    } else {
        None
    };

    let benchmark_stability_check = if cfg.benchmark_mode && cfg.benchmark_phase == "measured" {
        let samples = cfg
            .benchmark_stability_check_samples
            .unwrap_or(DEFAULT_STABILITY_CHECK_SAMPLES);
        let interval_ms = cfg
            .benchmark_stability_check_interval_ms
            .unwrap_or(DEFAULT_STABILITY_CHECK_INTERVAL_MS);
        let stability = measure_stability_check(&target, samples, interval_ms).await;
        match stability.as_ref() {
            Some(check) if check.successful_samples > 0 => {
                info!(
                    "Stability-check: {} | RTT avg={:.2}ms min={:.2}ms max={:.2}ms p50={:.2}ms p95={:.2}ms jitter={:.2}ms loss={:.1}% ({} / {} samples)",
                    check.network_type,
                    check.rtt_avg_ms,
                    check.rtt_min_ms,
                    check.rtt_max_ms,
                    check.rtt_p50_ms,
                    check.rtt_p95_ms,
                    check.jitter_ms,
                    check.packet_loss_percent,
                    check.successful_samples,
                    check.attempted_samples,
                );
            }
            Some(check) => {
                warn!(
                    "Stability-check: {} | RTT probes failed (loss={:.1}% across {} attempts)",
                    check.network_type, check.packet_loss_percent, check.attempted_samples,
                );
            }
            None => warn!("Could not perform benchmark stability-check"),
        }
        stability
    } else {
        None
    };

    // ── Measure network baseline RTT ────────────────────────────────────────
    let baseline = if let Some(environment_check) = benchmark_environment_check.as_ref() {
        Some(baseline_from_environment_check(environment_check))
    } else if let Some(stability_check) = benchmark_stability_check.as_ref() {
        Some(baseline_from_stability_check(stability_check))
    } else {
        match measure_baseline(&target).await {
            Some(bl) if bl.samples > 0 => {
                info!(
                "Network baseline: {} | RTT avg={:.2}ms min={:.2}ms max={:.2}ms p50={:.2}ms p95={:.2}ms ({} samples)",
                bl.network_type, bl.rtt_avg_ms, bl.rtt_min_ms, bl.rtt_max_ms,
                bl.rtt_p50_ms, bl.rtt_p95_ms, bl.samples,
            );
                Some(bl)
            }
            Some(bl) => {
                warn!(
                "Network baseline: {} | RTT probes failed (target may be unreachable) — network type still detected",
                bl.network_type,
            );
                Some(bl)
            }
            None => {
                warn!("Could not measure network baseline");
                None
            }
        }
    };

    let run_id = Uuid::new_v4();
    let started_at = Utc::now();

    info!(
        run_id  = %run_id,
        target  = %target_url_str,
        modes   = ?modes.iter().map(|m| m.to_string()).collect::<Vec<_>>(),
        runs    = cfg.runs,
        "Starting run"
    );

    // Explicit request body (apibench workloads): loaded exactly once here so
    // the per-request path only clones a refcounted `Bytes` handle.
    let request_body: Option<bytes::Bytes> = match (&cfg.request_body, &cfg.request_body_file) {
        (Some(inline), _) => Some(bytes::Bytes::from(inline.clone().into_bytes())),
        (None, Some(path)) => Some(bytes::Bytes::from(
            std::fs::read(path).with_context(|| format!("reading --request-body-file {path}"))?,
        )),
        (None, None) => None,
    };

    let probe_cfg = RunConfig {
        timeout_ms: cfg.timeout * 1000,
        dns_enabled: cfg.dns_enabled,
        ipv4_only: cfg.ipv4_only,
        ipv6_only: cfg.ipv6_only,
        insecure: cfg.insecure,
        payload_size: cfg.payload_size,
        path: target.path().to_string(),
        ca_bundle: cfg.ca_bundle.clone(),
        proxy: cfg.proxy.clone(),
        no_proxy: cfg.no_proxy,
        request_body,
        request_content_type: cfg.request_content_type.clone(),
        bearer_token: cfg.bearer_token.clone(),
    };

    let throughput_cfg = ThroughputConfig {
        run_cfg: probe_cfg.clone(),
        base_url: target.clone(),
    };

    let pageload_cfg = PageLoadConfig {
        run_cfg: probe_cfg.clone(),
        base_url: target.clone(),
        asset_sizes: cfg.page_asset_sizes.clone(),
        preset_name: cfg.page_preset_name.clone(),
    };

    // Expand modes × payload sizes into a flat task list.
    // All throughput modes generate one task per payload size.
    let mode_tasks: Vec<(Protocol, Option<usize>)> = modes
        .iter()
        .flat_map(|p| match p {
            Protocol::Download
            | Protocol::Download1
            | Protocol::Download2
            | Protocol::Download3
            | Protocol::Upload
            | Protocol::Upload1
            | Protocol::Upload2
            | Protocol::Upload3
            | Protocol::WebDownload
            | Protocol::WebUpload
            | Protocol::UdpDownload
            | Protocol::UdpUpload => payload_sizes
                .iter()
                .map(|&sz| (p.clone(), Some(sz)))
                .collect::<Vec<_>>(),
            other => vec![(other.clone(), None)],
        })
        .collect();

    let udp_cfg = UdpProbeConfig {
        target_host: target_host.clone(),
        target_port: cfg.udp_port,
        probe_count: cfg.udp_probes,
        timeout_ms: cfg.timeout * 1000,
        payload_size: 64,
    };

    let udp_throughput_cfg = UdpThroughputConfig {
        target_host: target_host.clone(),
        target_port: cfg.udp_throughput_port,
        timeout_ms: cfg.timeout * 1000,
    };

    // ── Connection reuse: warmup ──────────────────────────────────────────────
    let has_pageload2 = modes.iter().any(|m| matches!(m, Protocol::PageLoad2));
    #[cfg(feature = "http3")]
    let has_pageload3 = modes.iter().any(|m| matches!(m, Protocol::PageLoad3));

    // Initialized below after warmup when connection_reuse && has_pageload2
    let shared_h2: Option<std::sync::Arc<SharedH2Conn>> = None;
    #[cfg(feature = "http3")]
    let shared_h3: Option<
        std::sync::Arc<tokio::sync::Mutex<networker_tester::runner::pageload::SharedH3Conn>>,
    > = None;

    let retries = cfg.retries;
    let mut all_attempts = Vec::new();
    let mut seq = 0u32;

    // Warmup probes for connection-reuse modes
    let shared_h2 = if cfg.connection_reuse && has_pageload2 {
        info!("Connection reuse: warming up HTTP/2 connection…");
        let (warmup, conn) = warmup_pageload2(run_id, seq, &pageload_cfg).await;
        log_attempt(&warmup);
        all_attempts.push(warmup);
        seq += 1;
        conn.map(std::sync::Arc::new)
    } else {
        shared_h2
    };

    #[cfg(feature = "http3")]
    let shared_h3 = if cfg.connection_reuse && has_pageload3 {
        info!("Connection reuse: warming up HTTP/3 QUIC connection…");
        let (warmup, conn) = warmup_pageload3(run_id, seq, &pageload_cfg).await;
        log_attempt(&warmup);
        all_attempts.push(warmup);
        seq += 1;
        conn.map(|c| std::sync::Arc::new(tokio::sync::Mutex::new(c)))
    } else {
        shared_h3
    };
    #[cfg(not(feature = "http3"))]
    let shared_h3: Option<std::sync::Arc<tokio::sync::Mutex<()>>> = None;

    let connection_reuse_warmup_attempt_count = all_attempts.len() as u32;
    if cfg.concurrency > 1 {
        info!(
            concurrency = cfg.concurrency,
            "--concurrency > 1: attempts run concurrently within each probe mode; \
             different modes run sequentially so cross-mode contention cannot \
             distort latency or throughput readings"
        );
    }
    let collect_iteration = |seq: &mut u32| {
        let futures: Vec<(Protocol, _)> = mode_tasks
            .iter()
            .map(|(proto, payload_sz)| {
                let proto_tag = proto.clone();
                let proto = proto.clone();
                let payload_sz = *payload_sz;
                let target_clone = target.clone();
                let probe_cfg_clone = probe_cfg.clone();
                let udp_cfg_clone = udp_cfg.clone();
                let udp_throughput_cfg_clone = udp_throughput_cfg.clone();
                let throughput_cfg_clone = throughput_cfg.clone();
                let pageload_cfg_clone = pageload_cfg.clone();
                let shared_h2_clone = shared_h2.clone();
                let shared_h3_clone = shared_h3.clone();
                let current_seq = *seq;
                *seq += 1;

                let fut = async move {
                    macro_rules! do_dispatch {
                        () => {{
                            if matches!(proto, Protocol::PageLoad2) {
                                if let Some(ref h2) = shared_h2_clone {
                                    run_pageload2_warm(run_id, current_seq, &pageload_cfg_clone, h2)
                                        .await
                                } else {
                                    dispatch_once(
                                        &proto,
                                        payload_sz,
                                        run_id,
                                        current_seq,
                                        &target_clone,
                                        &cfg,
                                        &probe_cfg_clone,
                                        &udp_cfg_clone,
                                        &udp_throughput_cfg_clone,
                                        &throughput_cfg_clone,
                                        &pageload_cfg_clone,
                                    )
                                    .await
                                }
                            } else if matches!(proto, Protocol::PageLoad3)
                                && shared_h3_clone.is_some()
                            {
                                #[cfg(feature = "http3")]
                                {
                                    run_pageload3_warm(
                                        run_id,
                                        current_seq,
                                        &pageload_cfg_clone,
                                        shared_h3_clone.as_ref().unwrap(),
                                    )
                                    .await
                                }
                                #[cfg(not(feature = "http3"))]
                                {
                                    dispatch_once(
                                        &proto,
                                        payload_sz,
                                        run_id,
                                        current_seq,
                                        &target_clone,
                                        &cfg,
                                        &probe_cfg_clone,
                                        &udp_cfg_clone,
                                        &udp_throughput_cfg_clone,
                                        &throughput_cfg_clone,
                                        &pageload_cfg_clone,
                                    )
                                    .await
                                }
                            } else {
                                dispatch_once(
                                    &proto,
                                    payload_sz,
                                    run_id,
                                    current_seq,
                                    &target_clone,
                                    &cfg,
                                    &probe_cfg_clone,
                                    &udp_cfg_clone,
                                    &udp_throughput_cfg_clone,
                                    &throughput_cfg_clone,
                                    &pageload_cfg_clone,
                                )
                                .await
                            }
                        }};
                    }

                    let mut attempts = Vec::new();
                    let first_attempt = do_dispatch!();
                    attempts.push(first_attempt);

                    for retry_num in 1..=retries {
                        if attempts.last().is_some_and(|attempt| attempt.success) {
                            break;
                        }
                        let mut retry_attempt = do_dispatch!();
                        retry_attempt.retry_count = retry_num;
                        attempts.push(retry_attempt);
                    }

                    for attempt in &attempts {
                        log_attempt(attempt);
                    }

                    published_logical_attempts(attempts)
                };
                (proto_tag, fut)
            })
            .collect();

        async move {
            use futures::stream::{self, StreamExt};
            // Cross-mode isolation (V14): different probe types must never run
            // concurrently — a bulk download racing an http1 latency probe on
            // the same link produces self-induced queueing, and process-wide
            // CPU/context-switch counters cross-attribute each other's cost.
            // Group consecutive tasks by protocol: groups run strictly
            // sequentially; tasks within a group (payload-size variants of the
            // same mode) honor --concurrency.
            let mut groups: Vec<Vec<_>> = Vec::new();
            let mut last_proto: Option<Protocol> = None;
            for (proto, fut) in futures {
                if last_proto.as_ref() != Some(&proto) {
                    groups.push(Vec::new());
                    last_proto = Some(proto);
                }
                groups
                    .last_mut()
                    .expect("group pushed above is non-empty")
                    .push(fut);
            }
            let mut published: Vec<RequestAttempt> = Vec::new();
            for group in groups {
                let results: Vec<Vec<RequestAttempt>> = stream::iter(group)
                    .buffer_unordered(cfg.concurrency)
                    .collect()
                    .await;
                published.extend(results.into_iter().flatten());
            }
            published
        }
    };

    let pilot_criteria = benchmark_pilot_criteria(cfg);
    let pilot_adaptive_criteria = pilot_criteria.map(|criteria| BenchmarkAdaptiveCriteria {
        min_samples: criteria.min_samples,
        max_samples: criteria.max_samples,
        min_duration_ms: criteria.min_duration_ms,
        target_relative_error: None,
        target_absolute_error: None,
    });
    let overhead_runs = if cfg.benchmark_mode && cfg.benchmark_phase == "measured" {
        cfg.benchmark_overhead_samples
            .unwrap_or(DEFAULT_OVERHEAD_SAMPLES)
    } else {
        0
    };
    let cooldown_runs = if cfg.benchmark_mode && cfg.benchmark_phase == "measured" {
        cfg.benchmark_cooldown_samples
            .unwrap_or(DEFAULT_COOLDOWN_SAMPLES)
    } else {
        0
    };
    let mut overhead_attempts = Vec::new();
    let mut pilot_attempts = Vec::new();
    let mut pilot_completed_runs = 0u32;

    if overhead_runs > 0 {
        info!(samples = overhead_runs, "Benchmark overhead phase enabled");
        for overhead_index in 0..overhead_runs {
            info!("Overhead {}/{}", overhead_index + 1, overhead_runs);
            let published_results = collect_iteration(&mut seq).await;
            overhead_attempts.extend(published_results.iter().cloned());
            all_attempts.extend(published_results);
        }
    }

    if let Some(criteria) = pilot_adaptive_criteria {
        info!(
            min_samples = criteria.min_samples,
            max_samples = criteria.max_samples,
            min_duration_ms = criteria.min_duration_ms,
            "Pilot benchmark plan enabled"
        );
        while pilot_completed_runs < criteria.max_samples {
            info!(
                "Pilot {}/{}",
                pilot_completed_runs + 1,
                criteria.max_samples
            );
            let published_results = collect_iteration(&mut seq).await;
            pilot_attempts.extend(published_results.iter().cloned());
            all_attempts.extend(published_results);
            pilot_completed_runs += 1;

            let status = benchmark_adaptive_status(&criteria, &pilot_attempts);
            match status.stop_reason {
                Some(BenchmarkAdaptiveStopReason::AccuracyTargetReached) => {
                    info!(
                        completed_samples = status.completed_samples,
                        elapsed_ms = format_args!("{:.1}", status.elapsed_ms),
                        "Pilot benchmark sample budget satisfied"
                    );
                    break;
                }
                Some(BenchmarkAdaptiveStopReason::MaxSamplesReached) => {
                    info!(
                        completed_samples = status.completed_samples,
                        elapsed_ms = format_args!("{:.1}", status.elapsed_ms),
                        "Pilot benchmark reached maximum sample budget"
                    );
                    break;
                }
                None => {}
            }
        }
    }

    let benchmark_execution_plan = if !pilot_attempts.is_empty() {
        Some(derive_measured_plan_from_pilot(cfg, &pilot_attempts))
    } else if let Some(criteria) = benchmark_adaptive_criteria(cfg) {
        Some(BenchmarkExecutionPlan {
            source: "explicit".into(),
            min_samples: criteria.min_samples,
            max_samples: criteria.max_samples,
            min_duration_ms: criteria.min_duration_ms,
            target_relative_error: criteria.target_relative_error,
            target_absolute_error: criteria.target_absolute_error,
            pilot_sample_count: 0,
            pilot_elapsed_ms: None,
        })
    } else if cfg.benchmark_mode && cfg.benchmark_phase == "measured" {
        Some(BenchmarkExecutionPlan {
            source: "fixed-count".into(),
            min_samples: cfg.runs,
            max_samples: cfg.runs,
            min_duration_ms: 0,
            target_relative_error: None,
            target_absolute_error: None,
            pilot_sample_count: 0,
            pilot_elapsed_ms: None,
        })
    } else {
        None
    };
    let adaptive_criteria = benchmark_execution_plan
        .as_ref()
        .filter(|_| cfg.benchmark_mode && cfg.benchmark_phase == "measured")
        .map(|plan| BenchmarkAdaptiveCriteria {
            min_samples: plan.min_samples,
            max_samples: plan.max_samples,
            min_duration_ms: plan.min_duration_ms,
            target_relative_error: plan.target_relative_error,
            target_absolute_error: plan.target_absolute_error,
        });
    let max_run_count = adaptive_criteria.map_or(cfg.runs, |criteria| criteria.max_samples);
    if let Some(plan) = &benchmark_execution_plan {
        info!(
            source = %plan.source,
            min_samples = plan.min_samples,
            max_samples = plan.max_samples,
            min_duration_ms = plan.min_duration_ms,
            target_relative_error = plan.target_relative_error,
            target_absolute_error = plan.target_absolute_error,
            pilot_sample_count = plan.pilot_sample_count,
            "Benchmark measured execution plan resolved"
        );
    }
    let mut measured_attempts = Vec::new();
    let mut cooldown_attempts = Vec::new();
    let mut completed_runs = 0u32;
    let mut progress_request_counter = 0u32;
    let total_estimated_requests = max_run_count.saturating_mul(mode_tasks.len() as u32);

    while completed_runs < max_run_count {
        info!("Run {}/{}", completed_runs + 1, max_run_count);
        let published_results = collect_iteration(&mut seq).await;

        // Report progress for each measured attempt
        if let Some(ref reporter) = progress_reporter {
            for attempt in &published_results {
                progress_request_counter += 1;
                let r = reporter.clone();
                let mode = attempt.protocol.to_string();
                let lat = primary_metric_value(attempt).unwrap_or(0.0);
                let ok = attempt.success;
                let idx = progress_request_counter;
                let total = total_estimated_requests;
                tokio::spawn(async move {
                    r.report(&mode, idx, total, lat, ok).await;
                });
            }
        }

        measured_attempts.extend(published_results.iter().cloned());
        all_attempts.extend(published_results);
        completed_runs += 1;

        if let Some(criteria) = adaptive_criteria {
            let status = benchmark_adaptive_status(&criteria, &measured_attempts);
            match status.stop_reason {
                Some(BenchmarkAdaptiveStopReason::AccuracyTargetReached) => {
                    info!(
                        completed_samples = status.completed_samples,
                        elapsed_ms = format_args!("{:.1}", status.elapsed_ms),
                        "Adaptive benchmark stop criteria satisfied"
                    );
                    break;
                }
                Some(BenchmarkAdaptiveStopReason::MaxSamplesReached) => {
                    info!(
                        completed_samples = status.completed_samples,
                        elapsed_ms = format_args!("{:.1}", status.elapsed_ms),
                        "Adaptive benchmark reached maximum sample budget"
                    );
                    break;
                }
                None => {}
            }
        }
    }

    if cooldown_runs > 0 {
        info!(samples = cooldown_runs, "Benchmark cooldown phase enabled");
        for cooldown_index in 0..cooldown_runs {
            info!("Cooldown {}/{}", cooldown_index + 1, cooldown_runs);
            let published_results = collect_iteration(&mut seq).await;
            cooldown_attempts.extend(published_results.iter().cloned());
            all_attempts.extend(published_results);
        }
    }

    // ── HTTP stack comparison probes ──────────────────────────────────────────
    // For each configured HTTP stack (nginx, IIS, etc.), run pageload/browser
    // probes against the stack's ports and tag results with the stack name.
    let stack_modes: Vec<Protocol> = modes
        .iter()
        .filter(|m| {
            matches!(
                m,
                Protocol::PageLoad
                    | Protocol::PageLoad2
                    | Protocol::PageLoad3
                    | Protocol::Browser
                    | Protocol::Browser1
                    | Protocol::Browser2
                    | Protocol::Browser3
            )
        })
        .cloned()
        .collect();

    if !cfg.http_stacks.is_empty() && !stack_modes.is_empty() {
        for stack in &cfg.http_stacks {
            // Probe health endpoint to check if stack is running
            let stack_http_url = rewrite_url_for_stack(&target, stack.http_port, false);
            let health_url = {
                let mut u = stack_http_url.clone();
                u.set_path("/health");
                u
            };

            info!(stack = %stack.name, url = %health_url, "Checking HTTP stack availability");
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .danger_accept_invalid_certs(true)
                .build()
                .unwrap();
            match client.get(health_url.as_str()).send().await {
                Ok(resp) if resp.status().is_success() => {
                    info!(stack = %stack.name, "HTTP stack is available");
                }
                Ok(resp) => {
                    warn!(stack = %stack.name, status = %resp.status(), "HTTP stack returned non-OK; skipping");
                    continue;
                }
                Err(e) => {
                    warn!(stack = %stack.name, err = %e, "HTTP stack not reachable; skipping");
                    continue;
                }
            }

            // Build stack URLs: HTTP for pageload (H1.1), HTTPS for pageload2/3/browser.
            // Use root path "/" since nginx serves the test page there (not /health).
            let mut stack_https_url = rewrite_url_for_stack(&target, stack.https_port, true);
            stack_https_url.set_path("/");
            let mut stack_http_root = stack_http_url.clone();
            stack_http_root.set_path("/");

            // Build stack-specific configs (use HTTPS base for pageload2/3)
            let stack_pageload_cfg = PageLoadConfig {
                run_cfg: probe_cfg.clone(),
                base_url: stack_https_url.clone(),
                asset_sizes: cfg.page_asset_sizes.clone(),
                preset_name: cfg.page_preset_name.clone(),
            };
            // Separate config for pageload H1.1 (plain HTTP)
            let stack_pageload_h1_cfg = PageLoadConfig {
                run_cfg: probe_cfg.clone(),
                base_url: stack_http_root.clone(),
                asset_sizes: cfg.page_asset_sizes.clone(),
                preset_name: cfg.page_preset_name.clone(),
            };

            let stack_mode_tasks: Vec<(Protocol, Option<usize>)> =
                stack_modes.iter().map(|p| (p.clone(), None)).collect();

            for run_num in 0..cfg.runs {
                info!(stack = %stack.name, run = run_num + 1, "Stack probe run");

                for (proto, _) in &stack_mode_tasks {
                    // PageLoad (H1.1) uses plain HTTP; everything else uses HTTPS
                    let (stack_target, stack_pl_cfg) = if matches!(proto, Protocol::PageLoad) {
                        (&stack_http_root, &stack_pageload_h1_cfg)
                    } else {
                        (&stack_https_url, &stack_pageload_cfg)
                    };
                    let mut attempts = Vec::new();
                    let mut attempt = dispatch_once(
                        proto,
                        None,
                        run_id,
                        seq,
                        stack_target,
                        cfg,
                        &probe_cfg,
                        &udp_cfg,
                        &udp_throughput_cfg,
                        &throughput_cfg,
                        stack_pl_cfg,
                    )
                    .await;
                    seq += 1;

                    // Tag with the HTTP stack name
                    attempt.http_stack = Some(stack.name.clone());
                    attempts.push(attempt);

                    // Retry loop
                    for retry_num in 1..=retries {
                        if attempts.last().is_some_and(|candidate| candidate.success) {
                            break;
                        }
                        let mut retry_a = dispatch_once(
                            proto,
                            None,
                            run_id,
                            seq,
                            stack_target,
                            cfg,
                            &probe_cfg,
                            &udp_cfg,
                            &udp_throughput_cfg,
                            &throughput_cfg,
                            stack_pl_cfg,
                        )
                        .await;
                        seq += 1;
                        retry_a.retry_count = retry_num;
                        retry_a.http_stack = Some(stack.name.clone());
                        attempts.push(retry_a);
                    }

                    for attempt in &attempts {
                        log_attempt(attempt);
                    }

                    // Report progress for HTTP stack attempts
                    if let Some(ref reporter) = progress_reporter {
                        for attempt in &attempts {
                            progress_request_counter += 1;
                            let r = reporter.clone();
                            let mode = format!("{}:{}", attempt.protocol, stack.name);
                            let lat = primary_metric_value(attempt).unwrap_or(0.0);
                            let ok = attempt.success;
                            let idx = progress_request_counter;
                            let total = total_estimated_requests;
                            tokio::spawn(async move {
                                r.report(&mode, idx, total, lat, ok).await;
                            });
                        }
                    }

                    all_attempts.extend(published_logical_attempts(attempts));
                }
            }
        }
    }

    let finished_at = Utc::now();

    let benchmark_noise_thresholds = cfg.benchmark_mode.then(|| BenchmarkNoiseThresholds {
        max_packet_loss_percent: cfg
            .benchmark_max_packet_loss_percent
            .unwrap_or(BenchmarkNoiseThresholds::default().max_packet_loss_percent),
        max_jitter_ratio: cfg
            .benchmark_max_jitter_ratio
            .unwrap_or(BenchmarkNoiseThresholds::default().max_jitter_ratio),
        max_rtt_spread_ratio: cfg
            .benchmark_max_rtt_spread_ratio
            .unwrap_or(BenchmarkNoiseThresholds::default().max_rtt_spread_ratio),
    });

    let run = TestRun {
        schema_version: networker_tester::metrics::SCHEMA_VERSION.to_string(),
        run_id,
        started_at,
        finished_at: Some(finished_at),
        target_url: target_url_str.to_string(),
        target_host,
        modes: modes.iter().map(|m| m.to_string()).collect(),
        total_runs: completed_runs,
        concurrency: cfg.concurrency as u32,
        timeout_ms: cfg.timeout * 1000,
        client_os: std::env::consts::OS.to_string(),
        client_version: env!("CARGO_PKG_VERSION").to_string(),
        server_info,
        client_info,
        baseline,
        packet_capture_summary: None,
        benchmark_environment_check,
        benchmark_stability_check,
        benchmark_phase: cfg.benchmark_mode.then(|| cfg.benchmark_phase.clone()),
        benchmark_scenario: cfg.benchmark_mode.then(|| cfg.benchmark_scenario.clone()),
        benchmark_launch_index: cfg.benchmark_mode.then_some(cfg.benchmark_launch_index),
        benchmark_warmup_attempt_count: if cfg.benchmark_mode {
            connection_reuse_warmup_attempt_count
        } else {
            0
        },
        benchmark_pilot_attempt_count: if cfg.benchmark_mode {
            pilot_attempts.len().min(u32::MAX as usize) as u32
        } else {
            0
        },
        benchmark_overhead_attempt_count: if cfg.benchmark_mode {
            overhead_attempts.len().min(u32::MAX as usize) as u32
        } else {
            0
        },
        benchmark_cooldown_attempt_count: if cfg.benchmark_mode {
            cooldown_attempts.len().min(u32::MAX as usize) as u32
        } else {
            0
        },
        benchmark_execution_plan: cfg
            .benchmark_mode
            .then_some(benchmark_execution_plan)
            .flatten(),
        benchmark_noise_thresholds,
        attempts: all_attempts,
    };

    info!(
        success = run.success_count(),
        failure = run.failure_count(),
        target  = %target_url_str,
        "Run complete"
    );

    Ok(run)
}
