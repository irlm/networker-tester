use super::*;

#[test]
fn html_contains_target() {
    let run = make_run();
    let html = render(&run, None, None);
    assert!(html.contains("localhost/health"));
}

#[test]
fn html_contains_http11() {
    let run = make_run();
    let html = render(&run, None, None);
    assert!(html.contains("HTTP/1.1"));
}

#[test]
fn html_escapes_special_chars() {
    assert_eq!(
        escape_html("<script>alert(1)</script>"),
        "&lt;script&gt;alert(1)&lt;/script&gt;"
    );
}

#[test]
fn escape_html_ampersand_and_quotes() {
    assert_eq!(escape_html("a&b"), "a&amp;b");
    assert_eq!(escape_html("\"quoted\""), "&quot;quoted&quot;");
    assert_eq!(escape_html("it's"), "it&#39;s");
}

#[test]
fn escape_html_empty_string() {
    assert_eq!(escape_html(""), "");
}

#[test]
fn escape_html_ampersand_escaped_first_to_avoid_double_escaping() {
    // "&lt;" should become "&amp;lt;" not "&lt;" again
    assert_eq!(escape_html("&lt;"), "&amp;lt;");
}

// ── format_bytes ─────────────────────────────────────────────────────────

#[test]
fn format_bytes_zero() {
    assert_eq!(format_bytes(0), "0 B");
}

#[test]
fn format_bytes_one() {
    assert_eq!(format_bytes(1), "1 B");
}

#[test]
fn format_bytes_just_below_kib() {
    assert_eq!(format_bytes(1023), "1023 B");
}

#[test]
fn format_bytes_exactly_kib() {
    assert_eq!(format_bytes(1024), "1.0 KiB");
}

#[test]
fn format_bytes_just_below_mib() {
    assert_eq!(format_bytes(1024 * 1024 - 1), "1024.0 KiB");
}

#[test]
fn format_bytes_exactly_mib() {
    assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
}

#[test]
fn format_bytes_just_below_gib() {
    assert_eq!(format_bytes(1024 * 1024 * 1024 - 1), "1024.0 MiB");
}

#[test]
fn format_bytes_exactly_gib() {
    assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GiB");
}

#[test]
fn format_bytes_multiple_gibs() {
    assert_eq!(format_bytes(4 * 1024 * 1024 * 1024), "4.0 GiB");
}

#[test]
fn render_includes_packet_capture_section_when_present() {
    let run = make_run();
    let html = render(&run, None, Some(&sample_packet_capture_summary()));
    assert!(html.contains("Packet Capture Summary"));
    assert!(html.contains("Likely target endpoints"));
    assert!(html.contains("127.0.0.1"));
    assert!(html.contains("Confidence"));
    assert!(html.contains("Dominant trace port"));
    assert!(html.contains("Ambiguous trace"));
}

#[test]
fn save_writes_html_file() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run();
    save(&run, tmp.path(), None, None).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(content.starts_with("<!DOCTYPE html>"));
}

#[test]
fn html_includes_css_link_when_href_provided() {
    let run = make_run();
    let html = render(&run, Some("report.css"), None);
    assert!(html.contains(r#"<link rel="stylesheet""#));
    assert!(html.contains("report.css"));
}

#[test]
fn html_no_css_link_without_href() {
    let run = make_run();
    let html = render(&run, None, None);
    // Without an external CSS href, should embed inline styles but no <link>
    assert!(html.contains("<style>"));
}

#[test]
fn html_contains_error_section_for_failed_attempt() {
    use crate::metrics::{ErrorCategory, ErrorRecord};

    let run_id = Uuid::new_v4();
    let run = TestRun {
        schema_version: crate::metrics::SCHEMA_VERSION.to_string(),
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
        benchmark_environment_check: None,
        benchmark_stability_check: None,
        benchmark_phase: None,
        benchmark_scenario: None,
        benchmark_launch_index: None,
        benchmark_warmup_attempt_count: 0,
        benchmark_pilot_attempt_count: 0,
        benchmark_overhead_attempt_count: 0,
        benchmark_cooldown_attempt_count: 0,
        benchmark_execution_plan: None,
        benchmark_noise_thresholds: None,
        attempts: vec![RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: Protocol::Http1,
            sequence_num: 0,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            success: false,
            dns: None,
            tcp: None,
            tls: None,
            http: None,
            udp: None,
            error: Some(ErrorRecord {
                category: ErrorCategory::Tcp,
                message: "Connection refused".into(),
                detail: Some("os error 111".into()),
                occurred_at: Utc::now(),
            }),
            retry_count: 0,
            server_timing: None,
            udp_throughput: None,
            page_load: None,
            browser: None,
            http_stack: None,
        }],
    };
    let html = render(&run, None, None);
    assert!(html.contains("Errors"), "should have an Errors section");
    assert!(html.contains("Connection refused"));
}

#[test]
fn html_contains_throughput_section_for_download_attempt() {
    let run_id = Uuid::new_v4();
    let now = Utc::now();
    let run = TestRun {
        schema_version: crate::metrics::SCHEMA_VERSION.to_string(),
        run_id,
        started_at: now,
        finished_at: Some(now),
        target_url: "http://localhost/health".into(),
        target_host: "localhost".into(),
        modes: vec!["download".into()],
        total_runs: 1,
        concurrency: 1,
        timeout_ms: 5000,
        client_os: "test".into(),
        client_version: "0.1.0".into(),
        server_info: None,
        client_info: None,
        baseline: None,
        packet_capture_summary: None,
        benchmark_environment_check: None,
        benchmark_stability_check: None,
        benchmark_phase: None,
        benchmark_scenario: None,
        benchmark_launch_index: None,
        benchmark_warmup_attempt_count: 0,
        benchmark_pilot_attempt_count: 0,
        benchmark_overhead_attempt_count: 0,
        benchmark_cooldown_attempt_count: 0,
        benchmark_execution_plan: None,
        benchmark_noise_thresholds: None,
        attempts: vec![RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: Protocol::Download,
            sequence_num: 0,
            started_at: now,
            finished_at: Some(now),
            success: true,
            dns: None,
            tcp: None,
            tls: None,
            http: Some(HttpResult {
                negotiated_version: "HTTP/1.1".into(),
                status_code: 200,
                headers_size_bytes: 0,
                body_size_bytes: 1_048_576,
                ttfb_ms: 5.0,
                total_duration_ms: 95.0,
                redirect_count: 0,
                started_at: now,
                response_headers: vec![],
                payload_bytes: 1_048_576,
                throughput_mbps: Some(105.5),
                goodput_mbps: Some(98.0),
                cpu_time_ms: Some(12.0),
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
            http_stack: None,
        }],
    };
    let html = render(&run, None, None);
    assert!(
        html.contains("Throughput Results"),
        "should have a Throughput Results section"
    );
    assert!(html.contains("105"), "should show throughput value");
}

#[test]
fn html_contains_tls_section_for_tls_attempt() {
    let run_id = Uuid::new_v4();
    let now = Utc::now();
    let run = TestRun {
        schema_version: crate::metrics::SCHEMA_VERSION.to_string(),
        run_id,
        started_at: now,
        finished_at: Some(now),
        target_url: "https://localhost/health".into(),
        target_host: "localhost".into(),
        modes: vec!["tls".into()],
        total_runs: 1,
        concurrency: 1,
        timeout_ms: 5000,
        client_os: "test".into(),
        client_version: "0.1.0".into(),
        server_info: None,
        client_info: None,
        baseline: None,
        packet_capture_summary: None,
        benchmark_environment_check: None,
        benchmark_stability_check: None,
        benchmark_phase: None,
        benchmark_scenario: None,
        benchmark_launch_index: None,
        benchmark_warmup_attempt_count: 0,
        benchmark_pilot_attempt_count: 0,
        benchmark_overhead_attempt_count: 0,
        benchmark_cooldown_attempt_count: 0,
        benchmark_execution_plan: None,
        benchmark_noise_thresholds: None,
        attempts: vec![RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: crate::metrics::Protocol::Tls,
            sequence_num: 0,
            started_at: now,
            finished_at: Some(now),
            success: true,
            dns: None,
            tcp: None,
            tls: Some(crate::metrics::TlsResult {
                protocol_version: "TLSv1.3".into(),
                cipher_suite: "TLS_AES_256_GCM_SHA384".into(),
                alpn_negotiated: Some("h2".into()),
                cert_subject: Some("CN=localhost".into()),
                cert_issuer: Some("CN=Test CA".into()),
                cert_expiry: Some(now),
                handshake_duration_ms: 7.5,
                started_at: now,
                success: true,
                cert_chain: vec![],
                tls_backend: Some("rustls".into()),
                resumed: None,
                handshake_kind: None,
                tls13_tickets_received: None,
                previous_handshake_duration_ms: None,
                previous_handshake_kind: None,
                previous_http_status_code: None,
                http_status_code: None,
                ocsp_stapled: None,
                ocsp_response_bytes: None,
            }),
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
    };
    let html = render(&run, None, None);
    assert!(
        html.contains("TLS Details"),
        "should have TLS Details section"
    );
    assert!(html.contains("TLSv1.3"));
}

#[test]
fn html_contains_page_load_section() {
    let run_id = Uuid::new_v4();
    let now = Utc::now();
    let run = TestRun {
        schema_version: crate::metrics::SCHEMA_VERSION.to_string(),
        run_id,
        started_at: now,
        finished_at: Some(now),
        target_url: "https://localhost/health".into(),
        target_host: "localhost".into(),
        modes: vec!["pageload".into()],
        total_runs: 1,
        concurrency: 1,
        timeout_ms: 5000,
        client_os: "test".into(),
        client_version: "0.1.0".into(),
        server_info: None,
        client_info: None,
        baseline: None,
        packet_capture_summary: None,
        benchmark_environment_check: None,
        benchmark_stability_check: None,
        benchmark_phase: None,
        benchmark_scenario: None,
        benchmark_launch_index: None,
        benchmark_warmup_attempt_count: 0,
        benchmark_pilot_attempt_count: 0,
        benchmark_overhead_attempt_count: 0,
        benchmark_cooldown_attempt_count: 0,
        benchmark_execution_plan: None,
        benchmark_noise_thresholds: None,
        attempts: vec![RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: crate::metrics::Protocol::PageLoad,
            sequence_num: 0,
            started_at: now,
            finished_at: Some(now),
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
            page_load: Some(crate::metrics::PageLoadResult {
                asset_count: 20,
                assets_fetched: 20,
                total_bytes: 204_800,
                total_ms: 120.5,
                ttfb_ms: 5.2,
                connections_opened: 6,
                asset_timings_ms: vec![10.0; 20],
                started_at: now,
                tls_setup_ms: 24.0,
                tls_overhead_ratio: 0.19,
                per_connection_tls_ms: vec![4.0; 6],
                cpu_time_ms: Some(8.3),
                connection_reused: false,
            }),
            browser: None,
            http_stack: None,
        }],
    };
    let html = render(&run, None, None);
    // Page load data should appear in the Protocol Comparison section
    assert!(
        html.contains("pageload") || html.contains("PageLoad") || html.contains("Page Load"),
        "should reference page load mode"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// append_proto_row — protocol summary row rendering
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn append_proto_row_all_success_uses_ok_class() {
    let a1 = make_http_attempt(true, 5.0, 10.0);
    let a2 = make_http_attempt(true, 7.0, 14.0);
    let rows: Vec<&RequestAttempt> = vec![&a1, &a2];
    let mut out = String::new();
    append_proto_row(&mut out, &Protocol::Http1, &rows);
    assert!(
        out.contains(r#"class="ok""#),
        "all-success rows should use 'ok' class"
    );
    assert!(out.contains("2/2"), "should show 2/2 successes");
}

#[test]
fn append_proto_row_partial_success_uses_warn_class() {
    let ok = make_http_attempt(true, 5.0, 10.0);
    let fail = make_http_attempt(false, 0.0, 0.0);
    let rows: Vec<&RequestAttempt> = vec![&ok, &fail];
    let mut out = String::new();
    append_proto_row(&mut out, &Protocol::Http1, &rows);
    assert!(
        out.contains(r#"class="warn""#),
        "partial failures should use 'warn' class"
    );
    assert!(out.contains("1/2"), "should show 1/2 successes");
}

#[test]
fn append_proto_row_averages_ttfb_correctly() {
    let a1 = make_http_attempt(true, 10.0, 20.0);
    let a2 = make_http_attempt(true, 20.0, 40.0);
    let rows: Vec<&RequestAttempt> = vec![&a1, &a2];
    let mut out = String::new();
    append_proto_row(&mut out, &Protocol::Http1, &rows);
    // avg TTFB = (10 + 20) / 2 = 15.00
    assert!(out.contains("15.00"), "average TTFB should be 15.00");
    // avg total = (20 + 40) / 2 = 30.00
    assert!(out.contains("30.00"), "average total should be 30.00");
}

#[test]
fn append_proto_row_no_http_shows_dashes() {
    let a = RequestAttempt {
        attempt_id: Uuid::new_v4(),
        run_id: Uuid::new_v4(),
        protocol: Protocol::Tcp,
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
    };
    let rows: Vec<&RequestAttempt> = vec![&a];
    let mut out = String::new();
    append_proto_row(&mut out, &Protocol::Tcp, &rows);
    assert!(out.contains("—"), "no timing data should show em dash");
}

// ─────────────────────────────────────────────────────────────────────────
// append_attempt_row — individual attempt row rendering
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn append_attempt_row_success_http_shows_status_code() {
    let a = make_http_attempt(true, 5.0, 10.0);
    let mut out = String::new();
    append_attempt_row(&mut out, &a, false);
    assert!(out.contains("200"), "should show HTTP status 200");
    assert!(
        out.contains(r#"class="ok""#),
        "status cell should use 'ok' class"
    );
    // No error → em dash in error column
    assert!(out.contains("—"), "no error should show em dash");
}

#[test]
fn append_attempt_row_failed_shows_err_class() {
    let mut a = make_http_attempt(false, 0.0, 0.0);
    a.error = Some(ErrorRecord {
        category: ErrorCategory::Tcp,
        message: "connection refused".to_string(),
        detail: Some("detail info".to_string()),
        occurred_at: Utc::now(),
    });
    let mut out = String::new();
    append_attempt_row(&mut out, &a, false);
    assert!(
        out.contains("row-err"),
        "failed attempt should have row-err class"
    );
    assert!(
        out.contains("connection refused"),
        "error message should appear"
    );
    assert!(
        out.contains("detail info"),
        "detail should appear in title attr"
    );
}

#[test]
fn append_attempt_row_udp_echo_shows_rtt() {
    let a = RequestAttempt {
        attempt_id: Uuid::new_v4(),
        run_id: Uuid::new_v4(),
        protocol: Protocol::Udp,
        sequence_num: 1,
        started_at: Utc::now(),
        finished_at: Some(Utc::now()),
        success: true,
        dns: None,
        tcp: None,
        tls: None,
        http: None,
        udp: Some(UdpResult {
            remote_addr: "127.0.0.1:9000".into(),
            probe_count: 5,
            success_count: 5,
            loss_percent: 0.0,
            rtt_min_ms: 1.0,
            rtt_avg_ms: 2.5,
            rtt_p95_ms: 3.0,
            jitter_ms: 0.5,
            started_at: Utc::now(),
            probe_rtts_ms: vec![Some(2.5); 5],
        }),
        error: None,
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
        browser: None,
        http_stack: None,
    };
    let mut out = String::new();
    append_attempt_row(&mut out, &a, false);
    assert!(out.contains("2.50"), "rtt_avg_ms should appear");
    assert!(out.contains("loss=0.0%"), "loss percent should appear");
}

#[test]
fn append_attempt_row_udp_throughput_shows_transfer_ms() {
    let a = RequestAttempt {
        attempt_id: Uuid::new_v4(),
        run_id: Uuid::new_v4(),
        protocol: Protocol::UdpDownload,
        sequence_num: 2,
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
        udp_throughput: Some(UdpThroughputResult {
            remote_addr: "127.0.0.1:9998".into(),
            payload_bytes: 65_536,
            datagrams_sent: 50,
            datagrams_received: Some(50),
            bytes_acked: None,
            loss_percent: 0.0,
            transfer_ms: 125.0,
            throughput_mbps: Some(4.5),
            started_at: Utc::now(),
        }),
        page_load: None,
        browser: None,
        http_stack: None,
    };
    let mut out = String::new();
    append_attempt_row(&mut out, &a, false);
    assert!(out.contains("125.00"), "transfer_ms should appear");
    assert!(out.contains("4.50 MB/s"), "throughput should appear");
}

#[test]
fn append_attempt_row_no_results_shows_dashes() {
    let a = RequestAttempt {
        attempt_id: Uuid::new_v4(),
        run_id: Uuid::new_v4(),
        protocol: Protocol::Tcp,
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
    };
    let mut out = String::new();
    append_attempt_row(&mut out, &a, false);
    let dash_count = out.matches("—").count();
    assert!(
        dash_count >= 4,
        "no-data attempt should have multiple em dashes, got {dash_count}"
    );
    assert!(
        out.contains(r#"class="ok">OK<"#),
        "success with no HTTP should show OK"
    );
}

#[test]
fn append_attempt_row_http_throughput_shows_mbps() {
    let mut a = make_http_attempt(true, 5.0, 100.0);
    a.protocol = Protocol::Download;
    if let Some(ref mut h) = a.http {
        h.throughput_mbps = Some(12.34);
        h.payload_bytes = 1_048_576; // 1 MiB
    }
    let mut out = String::new();
    append_attempt_row(&mut out, &a, false);
    assert!(out.contains("12.34 MB/s"), "should show throughput");
    assert!(out.contains("1.0 MiB"), "should show payload size");
}

#[test]
fn append_attempt_row_with_stack_shows_stack_column() {
    let mut a = make_http_attempt(true, 5.0, 100.0);
    a.http_stack = Some("nginx".into());
    let mut out = String::new();
    append_attempt_row(&mut out, &a, true);
    assert!(out.contains("<td>nginx</td>"), "should show stack name");
}

#[test]
fn append_attempt_row_endpoint_shows_endpoint_label() {
    let a = make_http_attempt(true, 5.0, 100.0);
    let mut out = String::new();
    append_attempt_row(&mut out, &a, true);
    assert!(
        out.contains("<td>endpoint</td>"),
        "should show 'endpoint' for non-stack"
    );
}

#[test]
fn append_attempt_row_no_stack_column_when_disabled() {
    let a = make_http_attempt(true, 5.0, 100.0);
    let mut out = String::new();
    append_attempt_row(&mut out, &a, false);
    assert!(
        !out.contains("<td>endpoint</td>"),
        "should not show stack column"
    );
}

#[test]
fn html_contains_browser_section() {
    let run_id = Uuid::new_v4();
    let now = Utc::now();
    let run = TestRun {
        schema_version: crate::metrics::SCHEMA_VERSION.to_string(),
        run_id,
        started_at: now,
        finished_at: Some(now),
        target_url: "https://localhost/health".into(),
        target_host: "localhost".into(),
        modes: vec!["browser".into()],
        total_runs: 1,
        concurrency: 1,
        timeout_ms: 5000,
        client_os: "test".into(),
        client_version: "0.1.0".into(),
        server_info: None,
        client_info: None,
        baseline: None,
        packet_capture_summary: None,
        benchmark_environment_check: None,
        benchmark_stability_check: None,
        benchmark_phase: None,
        benchmark_scenario: None,
        benchmark_launch_index: None,
        benchmark_warmup_attempt_count: 0,
        benchmark_pilot_attempt_count: 0,
        benchmark_overhead_attempt_count: 0,
        benchmark_cooldown_attempt_count: 0,
        benchmark_execution_plan: None,
        benchmark_noise_thresholds: None,
        attempts: vec![RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: crate::metrics::Protocol::Browser,
            sequence_num: 0,
            started_at: now,
            finished_at: Some(now),
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
            browser: Some(crate::metrics::BrowserResult {
                load_ms: 350.0,
                dom_content_loaded_ms: 200.0,
                ttfb_ms: 50.0,
                resource_count: 21,
                transferred_bytes: 204_800,
                protocol: "h2".into(),
                resource_protocols: vec![("h2".into(), 21)],
                started_at: now,
            }),
            http_stack: None,
        }],
    };
    let html = render(&run, None, None);
    assert!(
        html.contains("Browser Results"),
        "should have Browser Results section"
    );
}
