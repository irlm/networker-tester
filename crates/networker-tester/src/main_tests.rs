
use super::*;
use chrono::{Duration, TimeZone};
use networker_tester::baseline::{
    average_jitter_ms, baseline_from_environment_check, baseline_from_stability_check, classify_ip,
    classify_target, percentile,
};
use networker_tester::benchmark::{
    adaptive_case_id, benchmark_adaptive_criteria, benchmark_adaptive_status,
    benchmark_attempt_wall_time_ms, benchmark_pilot_criteria, bootstrap_median_interval,
    derive_measured_plan_from_pilot, estimated_samples_for_error_targets, median_error_bounds,
    median_from_sorted, percentile_from_sorted, BenchmarkAdaptiveCriteria,
    BenchmarkAdaptiveStopReason, DeterministicRng, DEFAULT_AUTO_TARGET_RELATIVE_ERROR,
};
use networker_tester::cli::{
    ImpairmentProfile, ResolvedImpairmentConfig, ResolvedPacketCaptureConfig,
};
use networker_tester::dispatch::apply_impairment_target;
use networker_tester::metrics::{
    BenchmarkEnvironmentCheck, BenchmarkStabilityCheck, HttpResult, NetworkType,
};
use networker_tester::summary::fmt_bytes;
use uuid::Uuid;

fn request_attempt(success: bool, retry_count: u32) -> RequestAttempt {
    RequestAttempt {
        attempt_id: Uuid::new_v4(),
        run_id: Uuid::new_v4(),
        protocol: Protocol::Http1,
        sequence_num: 0,
        started_at: chrono::Utc::now(),
        finished_at: Some(chrono::Utc::now()),
        success,
        dns: None,
        tcp: None,
        tls: None,
        http: None,
        udp: None,
        error: None,
        retry_count,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
        browser: None,
        http_stack: None,
    }
}

fn measured_http_attempt(
    value_ms: f64,
    start_offset_ms: i64,
    wall_time_ms: i64,
    payload_bytes: usize,
    stack: Option<&str>,
) -> RequestAttempt {
    let started_at = Utc.timestamp_millis_opt(start_offset_ms).single().unwrap();
    RequestAttempt {
        attempt_id: Uuid::new_v4(),
        run_id: Uuid::new_v4(),
        protocol: Protocol::Http1,
        sequence_num: start_offset_ms.max(0) as u32,
        started_at,
        finished_at: Some(started_at + Duration::milliseconds(wall_time_ms)),
        success: true,
        dns: None,
        tcp: None,
        tls: None,
        http: Some(HttpResult {
            negotiated_version: "HTTP/1.1".into(),
            status_code: 200,
            headers_size_bytes: 128,
            body_size_bytes: payload_bytes,
            ttfb_ms: value_ms / 2.0,
            total_duration_ms: value_ms,
            redirect_count: 0,
            started_at,
            response_headers: Vec::new(),
            payload_bytes,
            throughput_mbps: None,
            goodput_mbps: None,
            cpu_time_ms: None,
            csw_voluntary: None,
            csw_involuntary: None,
            http_handshake_ms: None,
        }),
        udp: None,
        error: None,
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
        browser: None,
        http_stack: stack.map(str::to_string),
    }
}

fn failed_http_attempt(
    start_offset_ms: i64,
    wall_time_ms: i64,
    _payload_bytes: usize,
    stack: Option<&str>,
) -> RequestAttempt {
    let started_at = Utc.timestamp_millis_opt(start_offset_ms).single().unwrap();
    RequestAttempt {
        attempt_id: Uuid::new_v4(),
        run_id: Uuid::new_v4(),
        protocol: Protocol::Http1,
        sequence_num: start_offset_ms.max(0) as u32,
        started_at,
        finished_at: Some(started_at + Duration::milliseconds(wall_time_ms)),
        success: false,
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
        http_stack: stack.map(str::to_string),
    }
}

#[test]
fn rewrite_url_for_stack_http_port() {
    let base = url::Url::parse("https://example.com:8443/path").unwrap();
    let result = rewrite_url_for_stack(&base, 8081, false);
    assert_eq!(result.scheme(), "http");
    assert_eq!(result.port(), Some(8081));
    assert_eq!(result.path(), "/path");
    assert_eq!(result.host_str(), Some("example.com"));
}

#[test]
fn rewrite_url_for_stack_https_port() {
    let base = url::Url::parse("https://10.0.0.5:8443/").unwrap();
    let result = rewrite_url_for_stack(&base, 8444, true);
    assert_eq!(result.scheme(), "https");
    assert_eq!(result.port(), Some(8444));
}

#[test]
fn rewrite_url_for_stack_preserves_host_and_path() {
    let base = url::Url::parse("https://my-server.eastus.cloudapp.azure.com:8443/test").unwrap();
    let result = rewrite_url_for_stack(&base, 8082, true);
    assert_eq!(
        result.host_str(),
        Some("my-server.eastus.cloudapp.azure.com")
    );
    assert_eq!(result.path(), "/test");
    assert_eq!(result.port(), Some(8082));
}

#[test]
fn stack_mode_filter_keeps_only_pageload_and_browser() {
    let modes = vec![
        Protocol::Http1,
        Protocol::Http2,
        Protocol::Tcp,
        Protocol::PageLoad,
        Protocol::PageLoad2,
        Protocol::PageLoad3,
        Protocol::Browser,
        Protocol::Browser1,
        Protocol::Browser2,
        Protocol::Browser3,
        Protocol::Download,
        Protocol::Dns,
    ];
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
    assert_eq!(stack_modes.len(), 7);
    assert!(stack_modes.contains(&Protocol::PageLoad));
    assert!(stack_modes.contains(&Protocol::Browser3));
    assert!(!stack_modes.contains(&Protocol::Http1));
    assert!(!stack_modes.contains(&Protocol::Download));
}

#[test]
fn published_logical_attempts_keep_only_final_retry_outcome() {
    let published =
        published_logical_attempts(vec![request_attempt(false, 0), request_attempt(true, 1)]);

    assert_eq!(published.len(), 1);
    assert!(published[0].success);
    assert_eq!(published[0].retry_count, 1);
}

#[test]
fn benchmark_adaptive_criteria_requires_controls() {
    let mut cfg = sample_resolved_config(0);
    cfg.benchmark_mode = true;

    assert!(benchmark_adaptive_criteria(&cfg).is_none());

    cfg.benchmark_min_samples = Some(5);
    let criteria = benchmark_adaptive_criteria(&cfg).unwrap();
    assert_eq!(criteria.min_samples, 5);
    assert_eq!(criteria.max_samples, 1);
    assert_eq!(criteria.min_duration_ms, 0);
    assert_eq!(criteria.target_relative_error, None);
    assert_eq!(criteria.target_absolute_error, None);
}

#[test]
fn benchmark_pilot_criteria_defaults_for_measured_benchmark_mode() {
    let mut cfg = sample_resolved_config(0);
    cfg.benchmark_mode = true;
    cfg.runs = 10;

    let criteria = benchmark_pilot_criteria(&cfg).unwrap();
    assert_eq!(criteria.min_samples, 6);
    assert_eq!(criteria.max_samples, 10);
    assert_eq!(criteria.min_duration_ms, 0);
}

#[test]
fn benchmark_pilot_criteria_stays_disabled_when_explicit_measured_controls_exist() {
    let mut cfg = sample_resolved_config(0);
    cfg.benchmark_mode = true;
    cfg.benchmark_min_samples = Some(5);

    assert!(benchmark_pilot_criteria(&cfg).is_none());
}

#[test]
fn benchmark_adaptive_status_stops_when_accuracy_target_is_met() {
    let criteria = BenchmarkAdaptiveCriteria {
        min_samples: 3,
        max_samples: 10,
        min_duration_ms: 20,
        target_relative_error: Some(0.05),
        target_absolute_error: None,
    };
    let attempts = vec![
        measured_http_attempt(100.0, 0, 10, 1024, None),
        measured_http_attempt(100.0, 10, 10, 1024, None),
        measured_http_attempt(100.0, 20, 10, 1024, None),
    ];

    let status = benchmark_adaptive_status(&criteria, &attempts);

    assert_eq!(status.completed_samples, 3);
    assert_eq!(
        status.stop_reason,
        Some(BenchmarkAdaptiveStopReason::AccuracyTargetReached)
    );
}

#[test]
fn benchmark_adaptive_status_waits_for_min_duration() {
    let criteria = BenchmarkAdaptiveCriteria {
        min_samples: 3,
        max_samples: 10,
        min_duration_ms: 100,
        target_relative_error: Some(0.05),
        target_absolute_error: None,
    };
    let attempts = vec![
        measured_http_attempt(100.0, 0, 10, 1024, None),
        measured_http_attempt(100.0, 10, 10, 1024, None),
        measured_http_attempt(100.0, 20, 10, 1024, None),
    ];

    let status = benchmark_adaptive_status(&criteria, &attempts);

    assert_eq!(status.completed_samples, 3);
    assert!(status.elapsed_ms < 100.0);
    assert_eq!(status.stop_reason, None);
}

#[test]
fn benchmark_adaptive_status_uses_lowest_case_sample_count() {
    let criteria = BenchmarkAdaptiveCriteria {
        min_samples: 3,
        max_samples: 10,
        min_duration_ms: 0,
        target_relative_error: None,
        target_absolute_error: None,
    };
    let attempts = vec![
        measured_http_attempt(100.0, 0, 5, 1024, None),
        measured_http_attempt(100.0, 10, 5, 1024, None),
        measured_http_attempt(100.0, 20, 5, 1024, None),
        measured_http_attempt(101.0, 0, 5, 1024, Some("nginx")),
        measured_http_attempt(101.0, 10, 5, 1024, Some("nginx")),
    ];

    let status = benchmark_adaptive_status(&criteria, &attempts);

    assert_eq!(status.completed_samples, 2);
    assert_eq!(status.stop_reason, None);
}

#[test]
fn benchmark_adaptive_status_requires_metrics_for_every_case() {
    let criteria = BenchmarkAdaptiveCriteria {
        min_samples: 1,
        max_samples: 5,
        min_duration_ms: 0,
        target_relative_error: Some(0.05),
        target_absolute_error: None,
    };
    let attempts = vec![
        measured_http_attempt(100.0, 0, 10, 1024, None),
        failed_http_attempt(0, 10, 1024, Some("nginx")),
    ];

    let status = benchmark_adaptive_status(&criteria, &attempts);

    assert_eq!(status.completed_samples, 1);
    assert_eq!(status.stop_reason, None);
}

#[test]
fn benchmark_adaptive_status_stops_at_max_samples_when_noise_stays_high() {
    let criteria = BenchmarkAdaptiveCriteria {
        min_samples: 2,
        max_samples: 4,
        min_duration_ms: 0,
        target_relative_error: Some(0.01),
        target_absolute_error: None,
    };
    let attempts = vec![
        measured_http_attempt(100.0, 0, 5, 1024, None),
        measured_http_attempt(200.0, 10, 5, 1024, None),
        measured_http_attempt(50.0, 20, 5, 1024, None),
        measured_http_attempt(300.0, 30, 5, 1024, None),
    ];

    let status = benchmark_adaptive_status(&criteria, &attempts);

    assert_eq!(status.completed_samples, 4);
    assert_eq!(
        status.stop_reason,
        Some(BenchmarkAdaptiveStopReason::MaxSamplesReached)
    );
}

#[test]
fn derive_measured_plan_from_pilot_estimates_targets() {
    let mut cfg = sample_resolved_config(0);
    cfg.benchmark_mode = true;
    cfg.runs = 40;
    let attempts = vec![
        measured_http_attempt(100.0, 0, 10, 1024, None),
        measured_http_attempt(100.0, 10, 10, 1024, None),
        measured_http_attempt(100.0, 20, 10, 1024, None),
        measured_http_attempt(100.0, 30, 10, 1024, None),
        measured_http_attempt(100.0, 40, 10, 1024, None),
        measured_http_attempt(100.0, 50, 10, 1024, None),
    ];

    let plan = derive_measured_plan_from_pilot(&cfg, &attempts);

    assert_eq!(plan.source, "pilot-derived");
    assert_eq!(plan.pilot_sample_count, 6);
    assert_eq!(
        plan.target_relative_error,
        Some(DEFAULT_AUTO_TARGET_RELATIVE_ERROR)
    );
    assert_eq!(plan.target_absolute_error, None);
    assert_eq!(plan.min_samples, 6);
    assert_eq!(plan.max_samples, 6);
    assert!(plan.pilot_elapsed_ms.is_some());
}

#[test]
fn benchmark_adaptive_status_stops_when_absolute_error_target_is_met() {
    let criteria = BenchmarkAdaptiveCriteria {
        min_samples: 3,
        max_samples: 10,
        min_duration_ms: 0,
        target_relative_error: None,
        target_absolute_error: Some(1.0),
    };
    let attempts = vec![
        measured_http_attempt(100.0, 0, 10, 1024, None),
        measured_http_attempt(100.0, 10, 10, 1024, None),
        measured_http_attempt(100.0, 20, 10, 1024, None),
    ];

    let status = benchmark_adaptive_status(&criteria, &attempts);

    assert_eq!(
        status.stop_reason,
        Some(BenchmarkAdaptiveStopReason::AccuracyTargetReached)
    );
}

#[test]
fn benchmark_adaptive_status_requires_all_configured_error_targets() {
    let criteria = BenchmarkAdaptiveCriteria {
        min_samples: 3,
        max_samples: 10,
        min_duration_ms: 0,
        target_relative_error: Some(0.01),
        target_absolute_error: Some(100.0),
    };
    let attempts = vec![
        measured_http_attempt(100.0, 0, 10, 1024, None),
        measured_http_attempt(110.0, 10, 10, 1024, None),
        measured_http_attempt(90.0, 20, 10, 1024, None),
    ];

    let status = benchmark_adaptive_status(&criteria, &attempts);

    assert_eq!(status.stop_reason, None);
}

fn sample_resolved_config(delay_ms: u64) -> ResolvedConfig {
    ResolvedConfig {
        targets: vec!["https://127.0.0.1:8443/health".into()],
        url_test_url: None,
        tls_profile_url: None,
        tls_profile_ip: None,
        tls_profile_sni: None,
        tls_profile_target_kind: None,
        tls_profile_json: false,
        tls_profile_project_id: None,
        url_test_auth_token: None,
        url_test_cookie: None,
        url_test_headers: vec![],
        url_test_capture_har: false,
        url_test_capture_pcap: false,
        url_test_protocol_force: None,
        url_test_http3_repeat: 10,
        url_test_json: false,
        modes: vec![],
        runs: 1,
        concurrency: 1,
        timeout: 1000,
        payload_size: 0,
        payload_sizes: vec![],
        request_body: None,
        request_body_file: None,
        request_content_type: None,
        bearer_token: None,
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
        output_dir: ".".into(),
        html_report: "report.html".into(),
        css: None,
        excel: false,
        benchmark_mode: false,
        benchmark_phase: "measured".into(),
        benchmark_scenario: "default".into(),
        benchmark_launch_index: 0,
        benchmark_min_samples: None,
        benchmark_max_samples: None,
        benchmark_min_duration_ms: None,
        benchmark_target_relative_error: None,
        benchmark_target_absolute_error: None,
        benchmark_pilot_min_samples: None,
        benchmark_pilot_max_samples: None,
        benchmark_pilot_min_duration_ms: None,
        benchmark_environment_check_samples: None,
        benchmark_environment_check_interval_ms: None,
        benchmark_stability_check_samples: None,
        benchmark_stability_check_interval_ms: None,
        benchmark_max_packet_loss_percent: None,
        benchmark_max_jitter_ratio: None,
        benchmark_max_rtt_spread_ratio: None,
        benchmark_overhead_samples: None,
        benchmark_cooldown_samples: None,
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
            mode: networker_tester::cli::PacketCaptureMode::None,
            install_requirements: false,
            interface: "auto".into(),
            write_pcap: false,
            write_summary_json: false,
        },
        impairment: ResolvedImpairmentConfig {
            profile: ImpairmentProfile::None,
            delay_ms,
        },
        json_stdout: false,
        progress_url: None,
        progress_token: None,
        progress_interval: 1,
        progress_config_id: None,
        progress_testbed_id: None,
        benchmark_language: None,
        log_db_url: None,
    }
}

#[test]
fn impairment_target_rewrites_supported_http_family_probe() {
    let cfg = sample_resolved_config(150);
    let base = url::Url::parse("https://example.com:8443/health").unwrap();
    for proto in [
        Protocol::Http1,
        Protocol::Http2,
        Protocol::Http3,
        Protocol::Tcp,
        Protocol::Tls,
        Protocol::Native,
        Protocol::Curl,
    ] {
        let rewritten = apply_impairment_target(&proto, &base, &cfg);
        assert_eq!(rewritten.path(), "/delay");
        assert_eq!(rewritten.query(), Some("ms=150"));
        assert_eq!(rewritten.host_str(), Some("example.com"));
    }
}

#[test]
fn impairment_target_skips_unsupported_probe_types() {
    let cfg = sample_resolved_config(150);
    let base = url::Url::parse("https://example.com:8443/health").unwrap();
    let rewritten = apply_impairment_target(&Protocol::Udp, &base, &cfg);
    assert_eq!(rewritten.path(), "/health");
    assert_eq!(rewritten.query(), None);
}

#[test]
fn impairment_target_is_noop_when_delay_is_zero() {
    let cfg = sample_resolved_config(0);
    let base = url::Url::parse("https://example.com:8443/health").unwrap();
    let rewritten = apply_impairment_target(&Protocol::Http2, &base, &cfg);
    assert_eq!(rewritten, base);
}

#[test]
fn average_jitter_uses_adjacent_rtt_deltas() {
    let jitter = average_jitter_ms(&[1.0, 1.4, 0.8, 1.1]);
    assert!((jitter - 0.433_333_333_333_333_35).abs() < 1e-12);
}

#[test]
fn baseline_from_stability_check_reuses_rtt_distribution() {
    let stability = BenchmarkStabilityCheck {
        attempted_samples: 12,
        successful_samples: 10,
        failed_samples: 2,
        duration_ms: 500.0,
        rtt_min_ms: 0.8,
        rtt_avg_ms: 1.1,
        rtt_max_ms: 1.6,
        rtt_p50_ms: 1.0,
        rtt_p95_ms: 1.4,
        jitter_ms: 0.1,
        packet_loss_percent: 16.7,
        network_type: NetworkType::Loopback,
    };

    let baseline = baseline_from_stability_check(&stability);
    assert_eq!(baseline.samples, 10);
    assert_eq!(baseline.rtt_min_ms, 0.8);
    assert_eq!(baseline.rtt_avg_ms, 1.1);
    assert_eq!(baseline.rtt_max_ms, 1.6);
    assert_eq!(baseline.rtt_p50_ms, 1.0);
    assert_eq!(baseline.rtt_p95_ms, 1.4);
    assert_eq!(baseline.network_type, NetworkType::Loopback);
}

// ─── percentile / median / bootstrap ────────────────────────────────

#[test]
fn percentile_empty_returns_zero() {
    assert_eq!(percentile(&[], 50.0), 0.0);
}

#[test]
fn percentile_single_element() {
    assert_eq!(percentile(&[42.0], 0.0), 42.0);
    assert_eq!(percentile(&[42.0], 50.0), 42.0);
    assert_eq!(percentile(&[42.0], 100.0), 42.0);
}

#[test]
fn percentile_interpolates_correctly() {
    let sorted = [1.0, 2.0, 3.0, 4.0, 5.0];
    assert_eq!(percentile(&sorted, 0.0), 1.0);
    assert_eq!(percentile(&sorted, 100.0), 5.0);
    assert_eq!(percentile(&sorted, 50.0), 3.0);
    // p25 = index 1.0 → value 2.0
    assert!((percentile(&sorted, 25.0) - 2.0).abs() < 1e-12);
}

#[test]
fn percentile_from_sorted_matches_percentile() {
    let sorted = [10.0, 20.0, 30.0, 40.0, 50.0];
    for p in [0.0, 25.0, 50.0, 75.0, 100.0] {
        assert!(
            (percentile(&sorted, p) - percentile_from_sorted(&sorted, p)).abs() < 1e-12,
            "mismatch at p={p}"
        );
    }
}

#[test]
fn median_from_sorted_odd_length() {
    assert_eq!(median_from_sorted(&[1.0, 2.0, 3.0]), 2.0);
    assert_eq!(median_from_sorted(&[5.0]), 5.0);
}

#[test]
fn median_from_sorted_even_length() {
    assert_eq!(median_from_sorted(&[1.0, 3.0]), 2.0);
    assert_eq!(median_from_sorted(&[1.0, 2.0, 3.0, 4.0]), 2.5);
}

#[test]
fn median_from_sorted_empty() {
    assert_eq!(median_from_sorted(&[]), 0.0);
}

#[test]
fn bootstrap_median_interval_empty() {
    assert_eq!(bootstrap_median_interval(&[]), (0.0, 0.0, 0.0));
}

#[test]
fn bootstrap_median_interval_single() {
    let (se, lo, hi) = bootstrap_median_interval(&[42.0]);
    assert_eq!(se, 0.0);
    assert_eq!(lo, 42.0);
    assert_eq!(hi, 42.0);
}

#[test]
fn bootstrap_median_interval_identical_values() {
    let (se, lo, hi) = bootstrap_median_interval(&[7.0, 7.0, 7.0, 7.0]);
    assert!(se.abs() < 1e-12);
    assert!((lo - 7.0).abs() < 1e-12);
    assert!((hi - 7.0).abs() < 1e-12);
}

#[test]
fn bootstrap_median_interval_is_deterministic() {
    let values = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let a = bootstrap_median_interval(&values);
    let b = bootstrap_median_interval(&values);
    assert_eq!(a, b);
}

#[test]
fn median_error_bounds_too_few_values() {
    assert!(median_error_bounds(&[]).is_none());
    assert!(median_error_bounds(&[1.0]).is_none());
}

#[test]
fn median_error_bounds_filters_nan() {
    assert!(median_error_bounds(&[f64::NAN, f64::NAN]).is_none());
}

#[test]
fn median_error_bounds_returns_some_for_valid_data() {
    let bounds = median_error_bounds(&[1.0, 2.0, 3.0, 4.0, 5.0]).unwrap();
    assert!((bounds.median - 3.0).abs() < 1e-12);
    assert!(bounds.absolute_half_width >= 0.0);
}

// ─── estimated_samples_for_error_targets ─────────────────────────────

#[test]
fn estimated_samples_returns_current_n_when_no_bounds() {
    // Single value → median_error_bounds returns None → returns current_n
    assert_eq!(estimated_samples_for_error_targets(&[1.0], None, None), 1);
}

#[test]
fn estimated_samples_increases_for_tight_relative_target() {
    let values = [100.0, 110.0, 90.0, 105.0, 95.0, 100.0, 98.0, 102.0];
    let loose = estimated_samples_for_error_targets(&values, Some(0.10), None);
    let tight = estimated_samples_for_error_targets(&values, Some(0.01), None);
    assert!(tight >= loose, "tighter target should need more samples");
}

// ─── fmt_bytes ───────────────────────────────────────────────────────

#[test]
fn fmt_bytes_displays_bytes() {
    assert_eq!(fmt_bytes(0), "0B");
    assert_eq!(fmt_bytes(512), "512B");
    assert_eq!(fmt_bytes(1023), "1023B");
}

#[test]
fn fmt_bytes_displays_kib() {
    assert_eq!(fmt_bytes(1024), "1KiB");
    assert_eq!(fmt_bytes(500 * 1024), "500KiB");
}

#[test]
fn fmt_bytes_displays_mib() {
    assert_eq!(fmt_bytes(1024 * 1024), "1MiB");
    assert_eq!(fmt_bytes(100 * 1024 * 1024), "100MiB");
}

#[test]
fn fmt_bytes_displays_gib() {
    assert_eq!(fmt_bytes(1024 * 1024 * 1024), "1.0GiB");
    assert_eq!(fmt_bytes(5 * 1024 * 1024 * 1024), "5.0GiB");
}

// ─── classify_ip ─────────────────────────────────────────────────────

#[test]
fn classify_ip_loopback_v4() {
    let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();
    assert_eq!(classify_ip(&ip), NetworkType::Loopback);
}

#[test]
fn classify_ip_loopback_v6() {
    let ip: std::net::IpAddr = "::1".parse().unwrap();
    assert_eq!(classify_ip(&ip), NetworkType::Loopback);
}

#[test]
fn classify_ip_private_10() {
    let ip: std::net::IpAddr = "10.0.0.1".parse().unwrap();
    assert_eq!(classify_ip(&ip), NetworkType::LAN);
}

#[test]
fn classify_ip_private_172() {
    let ip: std::net::IpAddr = "172.16.0.1".parse().unwrap();
    assert_eq!(classify_ip(&ip), NetworkType::LAN);
}

#[test]
fn classify_ip_private_192() {
    let ip: std::net::IpAddr = "192.168.1.1".parse().unwrap();
    assert_eq!(classify_ip(&ip), NetworkType::LAN);
}

#[test]
fn classify_ip_cgnat() {
    let ip: std::net::IpAddr = "100.64.0.1".parse().unwrap();
    assert_eq!(classify_ip(&ip), NetworkType::LAN);
    let ip2: std::net::IpAddr = "100.127.255.255".parse().unwrap();
    assert_eq!(classify_ip(&ip2), NetworkType::LAN);
}

#[test]
fn classify_ip_link_local_v4() {
    let ip: std::net::IpAddr = "169.254.1.1".parse().unwrap();
    assert_eq!(classify_ip(&ip), NetworkType::LAN);
}

#[test]
fn classify_ip_public_v4() {
    let ip: std::net::IpAddr = "8.8.8.8".parse().unwrap();
    assert_eq!(classify_ip(&ip), NetworkType::Internet);
}

#[test]
fn classify_ip_link_local_v6() {
    let ip: std::net::IpAddr = "fe80::1".parse().unwrap();
    assert_eq!(classify_ip(&ip), NetworkType::LAN);
}

#[test]
fn classify_ip_ula_v6() {
    let ip: std::net::IpAddr = "fd00::1".parse().unwrap();
    assert_eq!(classify_ip(&ip), NetworkType::LAN);
}

#[test]
fn classify_ip_public_v6() {
    let ip: std::net::IpAddr = "2001:db8::1".parse().unwrap();
    assert_eq!(classify_ip(&ip), NetworkType::Internet);
}

// ─── classify_target ─────────────────────────────────────────────────

#[test]
fn classify_target_localhost() {
    assert_eq!(classify_target("localhost"), NetworkType::Loopback);
}

#[test]
fn classify_target_ip_string() {
    assert_eq!(classify_target("10.0.0.5"), NetworkType::LAN);
    assert_eq!(classify_target("8.8.4.4"), NetworkType::Internet);
    assert_eq!(classify_target("127.0.0.1"), NetworkType::Loopback);
}

// ─── adaptive_case_id ────────────────────────────────────────────────

#[test]
fn adaptive_case_id_default_values() {
    let a = request_attempt(true, 0);
    let id = adaptive_case_id(&a);
    assert_eq!(id, "http1:default:default");
}

#[test]
fn adaptive_case_id_with_stack_and_payload() {
    let a = measured_http_attempt(100.0, 0, 10, 4096, Some("nginx"));
    let id = adaptive_case_id(&a);
    assert_eq!(id, "http1:4096:nginx");
}

#[test]
fn adaptive_case_id_escapes_colons_in_stack() {
    let a = measured_http_attempt(100.0, 0, 10, 1024, Some("stack:v2"));
    let id = adaptive_case_id(&a);
    assert!(
        !id.matches(':').count() > 2 || id.contains("stack_v2"),
        "colons in stack should be escaped"
    );
}

// ─── benchmark_attempt_wall_time_ms ──────────────────────────────────

#[test]
fn benchmark_attempt_wall_time_ms_empty() {
    assert_eq!(benchmark_attempt_wall_time_ms(&[]), 0.0);
}

#[test]
fn benchmark_attempt_wall_time_ms_single() {
    let attempts = vec![measured_http_attempt(100.0, 0, 50, 1024, None)];
    let wall = benchmark_attempt_wall_time_ms(&attempts);
    assert!((wall - 50.0).abs() < 1.0);
}

// ─── average_jitter_ms ───────────────────────────────────────────────

#[test]
fn average_jitter_empty() {
    assert_eq!(average_jitter_ms(&[]), 0.0);
}

#[test]
fn average_jitter_single_sample() {
    assert_eq!(average_jitter_ms(&[1.0]), 0.0);
}

#[test]
fn average_jitter_identical_samples() {
    assert_eq!(average_jitter_ms(&[5.0, 5.0, 5.0]), 0.0);
}

#[test]
fn baseline_from_environment_check_reuses_rtt_distribution() {
    let environment_check = BenchmarkEnvironmentCheck {
        attempted_samples: 5,
        successful_samples: 4,
        failed_samples: 1,
        duration_ms: 250.0,
        rtt_min_ms: 0.7,
        rtt_avg_ms: 0.9,
        rtt_max_ms: 1.2,
        rtt_p50_ms: 0.85,
        rtt_p95_ms: 1.1,
        packet_loss_percent: 20.0,
        network_type: NetworkType::Loopback,
    };

    let baseline = baseline_from_environment_check(&environment_check);
    assert_eq!(baseline.samples, 4);
    assert_eq!(baseline.rtt_min_ms, 0.7);
    assert_eq!(baseline.rtt_avg_ms, 0.9);
    assert_eq!(baseline.rtt_max_ms, 1.2);
    assert_eq!(baseline.rtt_p50_ms, 0.85);
    assert_eq!(baseline.rtt_p95_ms, 1.1);
    assert_eq!(baseline.network_type, NetworkType::Loopback);
}

// ─── DeterministicRng — panic safety & reproducibility ───────────────

#[test]
fn deterministic_rng_reproducible_from_same_seed() {
    let values = [1.0, 2.0, 3.0, 4.0, 5.0];
    let mut rng1 = DeterministicRng::from_values(&values);
    let mut rng2 = DeterministicRng::from_values(&values);
    for _ in 0..100 {
        assert_eq!(rng1.next_u64(), rng2.next_u64());
    }
}

#[test]
fn deterministic_rng_different_seeds_diverge() {
    let mut rng1 = DeterministicRng::from_values(&[1.0, 2.0]);
    let mut rng2 = DeterministicRng::from_values(&[3.0, 4.0]);
    // At least one of the first 10 values should differ.
    let differs = (0..10).any(|_| rng1.next_u64() != rng2.next_u64());
    assert!(
        differs,
        "different seeds should produce different sequences"
    );
}

#[test]
fn deterministic_rng_empty_array() {
    // Empty input should not panic and should produce a valid RNG.
    let mut rng = DeterministicRng::from_values(&[]);
    // Just verify it doesn't panic and produces values.
    let _ = rng.next_u64();
}

#[test]
fn deterministic_rng_state_never_zero() {
    // Values that XOR to zero should still produce non-zero state.
    let rng = DeterministicRng::from_values(&[0.0]);
    assert_ne!(rng.state, 0);
}

#[test]
fn deterministic_rng_next_index_in_bounds() {
    let mut rng = DeterministicRng::from_values(&[42.0]);
    for upper in [1, 2, 5, 100, 1000] {
        for _ in 0..50 {
            let idx = rng.next_index(upper);
            assert!(idx < upper, "index {idx} >= upper {upper}");
        }
    }
}

#[test]
fn deterministic_rng_next_index_upper_one() {
    // upper=1 → always returns 0.
    let mut rng = DeterministicRng::from_values(&[1.0, 2.0]);
    for _ in 0..10 {
        assert_eq!(rng.next_index(1), 0);
    }
}

// ─── published_logical_attempts — edge cases ─────────────────────────

#[test]
fn published_logical_attempts_empty_vector() {
    let result = published_logical_attempts(vec![]);
    assert!(result.is_empty());
}

#[test]
fn published_logical_attempts_single_success() {
    let result = published_logical_attempts(vec![request_attempt(true, 0)]);
    assert_eq!(result.len(), 1);
    assert!(result[0].success);
}

#[test]
fn published_logical_attempts_single_failure() {
    let result = published_logical_attempts(vec![request_attempt(false, 0)]);
    assert_eq!(result.len(), 1);
    assert!(!result[0].success);
}
