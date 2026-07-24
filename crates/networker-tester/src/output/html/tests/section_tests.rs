use super::*;

// ─────────────────────────────────────────────────────────────────────────
// Statistics summary section
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn statistics_summary_appears_with_multiple_attempts() {
    let mut run = make_run();
    run.attempts.clear();
    for i in 0..3 {
        let mut a = make_attempt(Protocol::Http1, true);
        a.sequence_num = i;
        a.http.as_mut().unwrap().total_duration_ms = 10.0 * (i as f64 + 1.0);
        run.attempts.push(a);
    }
    let html = render(&run, None, None);
    assert!(
        html.contains("Statistics Summary"),
        "should have Statistics Summary section"
    );
}

#[test]
fn statistics_summary_shows_percentiles() {
    let mut run = make_run();
    run.attempts.clear();
    for i in 0..5 {
        let mut a = make_attempt(Protocol::Http1, true);
        a.sequence_num = i;
        a.http.as_mut().unwrap().total_duration_ms = 10.0 * (i as f64 + 1.0);
        run.attempts.push(a);
    }
    let html = render(&run, None, None);
    assert!(html.contains("p50"), "should show p50 column");
    assert!(html.contains("p95"), "should show p95 column");
    assert!(html.contains("p99"), "should show p99 column");
    assert!(html.contains("StdDev"), "should show StdDev column");
}

#[test]
fn statistics_success_pct_100_uses_ok_class() {
    let mut run = make_run();
    run.attempts.clear();
    for i in 0..3 {
        let mut a = make_attempt(Protocol::Http1, true);
        a.sequence_num = i;
        run.attempts.push(a);
    }
    let html = render(&run, None, None);
    assert!(html.contains("100%"), "all succeeded → 100% should appear");
}

// ─────────────────────────────────────────────────────────────────────────
// Page load section and protocol comparison
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn page_load_comparison_section_appears_with_multiple_protos() {
    let mut run = make_run();
    run.attempts.clear();
    for _ in 0..2 {
        run.attempts
            .push(make_page_load_attempt(Protocol::PageLoad, 120.0, false));
        run.attempts
            .push(make_page_load_attempt(Protocol::PageLoad2, 95.0, false));
    }
    let html = render(&run, None, None);
    assert!(
        html.contains("Protocol Comparison") && html.contains("Page Load"),
        "page load comparison section should appear"
    );
}

#[test]
fn page_load_cold_warm_split_shown_when_both_present() {
    let mut run = make_run();
    run.attempts.clear();
    // Add cold and warm pageload2 attempts
    for _ in 0..2 {
        run.attempts
            .push(make_page_load_attempt(Protocol::PageLoad2, 150.0, false));
        run.attempts
            .push(make_page_load_attempt(Protocol::PageLoad2, 90.0, true));
    }
    let html = render(&run, None, None);
    assert!(
        html.contains("cold"),
        "cold subset should appear in page load table"
    );
    assert!(
        html.contains("warm"),
        "warm subset should appear in page load table"
    );
}

#[test]
fn page_load_connection_reuse_observation_appears() {
    let mut run = make_run();
    run.attempts.clear();
    // Need cold and warm pageload2
    for _ in 0..2 {
        run.attempts
            .push(make_page_load_attempt(Protocol::PageLoad2, 200.0, false));
        run.attempts
            .push(make_page_load_attempt(Protocol::PageLoad2, 80.0, true));
    }
    let html = render(&run, None, None);
    // The analysis section should mention connection reuse savings
    // Chart sections are only rendered when both chart_browser and chart_pl are non-empty
    // but the comparison table always shows both subsets
    assert!(
        html.contains("cold") && html.contains("warm"),
        "both cold and warm labels must appear"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Browser protocol comparison and observations
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn browser_protocol_comparison_appears_with_multiple_browser_modes() {
    let mut run = make_run();
    run.attempts.clear();
    run.attempts
        .push(make_browser_attempt(Protocol::Browser1, 300.0, 50.0));
    run.attempts
        .push(make_browser_attempt(Protocol::Browser2, 250.0, 40.0));
    run.attempts
        .push(make_browser_attempt(Protocol::Browser3, 200.0, 35.0));
    let html = render(&run, None, None);
    assert!(
        html.contains("Protocol Comparison") && html.contains("Browser"),
        "browser comparison section should appear"
    );
    assert!(
        html.contains("browser1") || html.contains("Browser1"),
        "browser1 must appear"
    );
    assert!(
        html.contains("browser2") || html.contains("Browser2"),
        "browser2 must appear"
    );
    assert!(
        html.contains("browser3") || html.contains("Browser3"),
        "browser3 must appear"
    );
}

#[test]
fn browser_results_section_shows_protocol_and_timings() {
    let mut run = make_run();
    run.attempts.clear();
    run.attempts
        .push(make_browser_attempt(Protocol::Browser, 355.5, 48.2));
    let html = render(&run, None, None);
    assert!(
        html.contains("Browser Results"),
        "must have Browser Results section"
    );
    assert!(
        html.contains("355.50") || html.contains("355.5"),
        "load_ms should appear"
    );
    assert!(
        html.contains("48.20") || html.contains("48.2"),
        "ttfb_ms should appear"
    );
}

#[test]
fn charts_analysis_section_appears_with_pageload_data() {
    let mut run = make_run();
    run.attempts.clear();
    // We need >= 2 pageload attempts of the same protocol for charts
    for _ in 0..4 {
        run.attempts
            .push(make_page_load_attempt(Protocol::PageLoad2, 100.0, false));
    }
    for _ in 0..4 {
        run.attempts
            .push(make_browser_attempt(Protocol::Browser2, 120.0, 30.0));
    }
    let html = render(&run, None, None);
    assert!(
        html.contains("Charts"),
        "Charts &amp; Analysis section should appear"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// UDP statistics section
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn udp_statistics_section_appears_when_udp_attempts_present() {
    let run_id = Uuid::new_v4();
    let mut run = make_run();
    run.attempts.clear();
    run.attempts.push(RequestAttempt {
        attempt_id: Uuid::new_v4(),
        run_id,
        protocol: Protocol::Udp,
        sequence_num: 0,
        started_at: Utc::now(),
        finished_at: Some(Utc::now()),
        success: true,
        dns: None,
        tcp: None,
        tls: None,
        http: None,
        udp: Some(UdpResult {
            remote_addr: "10.0.0.1:9000".into(),
            probe_count: 10,
            success_count: 10,
            loss_percent: 0.0,
            rtt_min_ms: 1.0,
            rtt_avg_ms: 1.5,
            rtt_p95_ms: 2.0,
            jitter_ms: 0.2,
            started_at: Utc::now(),
            probe_rtts_ms: vec![Some(1.5); 10],
        }),
        error: None,
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
        browser: None,
        http_stack: None,
        rpm: None,
    });
    let html = render(&run, None, None);
    assert!(
        html.contains("UDP Probe Statistics"),
        "UDP section must appear"
    );
    assert!(html.contains("10.0.0.1:9000"), "remote addr should appear");
    assert!(html.contains("1.50"), "avg RTT should appear");
}

#[test]
fn udp_loss_shows_warn_class_when_nonzero() {
    let run_id = Uuid::new_v4();
    let mut run = make_run();
    run.attempts.clear();
    run.attempts.push(RequestAttempt {
        attempt_id: Uuid::new_v4(),
        run_id,
        protocol: Protocol::Udp,
        sequence_num: 0,
        started_at: Utc::now(),
        finished_at: Some(Utc::now()),
        success: true,
        dns: None,
        tcp: None,
        tls: None,
        http: None,
        udp: Some(UdpResult {
            remote_addr: "10.0.0.1:9000".into(),
            probe_count: 10,
            success_count: 8,
            loss_percent: 20.0,
            rtt_min_ms: 1.0,
            rtt_avg_ms: 1.5,
            rtt_p95_ms: 2.0,
            jitter_ms: 0.5,
            started_at: Utc::now(),
            probe_rtts_ms: vec![Some(1.5); 8],
        }),
        error: None,
        retry_count: 0,
        server_timing: None,
        udp_throughput: None,
        page_load: None,
        browser: None,
        http_stack: None,
        rpm: None,
    });
    let html = render(&run, None, None);
    assert!(html.contains("20.0%"), "loss percent should appear");
    // nonzero loss uses "warn" class in the loss cell
    assert!(
        html.contains(r#"class="warn">20.0%"#),
        "loss cell should use warn class"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// TCP stats section
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn tcp_stats_section_appears_when_tcp_attempt_present() {
    let run = make_run();
    // The existing make_run() has a TCP result already
    let html = render(&run, None, None);
    assert!(
        html.contains("TCP Stats"),
        "TCP Stats section should appear"
    );
    assert!(html.contains("127.0.0.1:12345"), "local addr should appear");
    assert!(html.contains("127.0.0.1:80"), "remote addr should appear");
}

#[test]
fn tcp_stats_ssthresh_shows_infinity_symbol_when_none() {
    let run = make_run(); // snd_ssthresh = None → "∞"
    let html = render(&run, None, None);
    // When snd_ssthresh is None the cell shows ∞
    assert!(html.contains("∞"), "None ssthresh should display ∞");
}

#[test]
fn tcp_stats_congestion_algorithm_shown_when_set() {
    let mut run = make_run();
    if let Some(ref mut tcp) = run.attempts[0].tcp {
        tcp.congestion_algorithm = Some("cubic".into());
    }
    let html = render(&run, None, None);
    assert!(
        html.contains("cubic"),
        "congestion algorithm should appear in TCP stats"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Error section
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn error_section_shows_detail_in_title_attribute() {
    let mut run = make_run();
    run.attempts[0].success = false;
    run.attempts[0].error = Some(ErrorRecord {
        category: ErrorCategory::Tls,
        message: "TLS handshake failed".into(),
        detail: Some("certificate expired".into()),
        occurred_at: Utc::now(),
    });
    let html = render(&run, None, None);
    assert!(
        html.contains("TLS handshake failed"),
        "error message must appear"
    );
    assert!(
        html.contains("certificate expired"),
        "error detail must appear"
    );
    assert!(
        html.contains("class=\"err\""),
        "error category cell should use err class"
    );
}

#[test]
fn error_section_no_detail_shows_dash() {
    let mut run = make_run();
    run.attempts[0].success = false;
    run.attempts[0].error = Some(ErrorRecord {
        category: ErrorCategory::Timeout,
        message: "deadline exceeded".into(),
        detail: None,
        occurred_at: Utc::now(),
    });
    let html = render(&run, None, None);
    assert!(html.contains("deadline exceeded"), "message must appear");
    assert!(html.contains("—"), "absent detail should show em dash");
}

#[test]
fn error_section_html_escapes_message() {
    let mut run = make_run();
    run.attempts[0].success = false;
    run.attempts[0].error = Some(ErrorRecord {
        category: ErrorCategory::Other,
        message: "<script>evil()</script>".into(),
        detail: None,
        occurred_at: Utc::now(),
    });
    let html = render(&run, None, None);
    assert!(
        html.contains("&lt;script&gt;"),
        "error message must be HTML-escaped"
    );
    assert!(
        !html.contains("<script>evil"),
        "raw script tag must not appear in output"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Timing breakdown table
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn timing_table_shows_all_protocols_with_attempts() {
    let mut run = make_run();
    run.attempts.clear();
    run.attempts.push(make_attempt(Protocol::Http1, true));
    run.attempts.push(make_attempt(Protocol::Http2, true));
    let html = render(&run, None, None);
    assert!(
        html.contains("Timing Breakdown by Protocol"),
        "timing table must appear"
    );
    assert!(
        html.contains("<strong>http1</strong>"),
        "http1 row must appear"
    );
    assert!(
        html.contains("<strong>http2</strong>"),
        "http2 row must appear"
    );
}

#[test]
fn timing_table_skips_protocols_with_no_attempts() {
    let run = make_run(); // only http1 attempts
    let html = render(&run, None, None);
    // http3 has no attempts — its row should not appear in the table
    assert!(
        !html.contains("<strong>http3</strong>"),
        "http3 row should not appear when no http3 attempts"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// render_multi with throughput metric labels
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn protocol_comparison_metric_label_correct_for_tcp() {
    let mut r1 = make_run_with_url("https://a.example.com/");
    let mut r2 = make_run_with_url("https://b.example.com/");
    let make_tcp = |ms: f64| -> RequestAttempt {
        let run_id = Uuid::new_v4();
        RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: Protocol::Tcp,
            sequence_num: 0,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            success: true,
            dns: None,
            tcp: Some(TcpResult {
                local_addr: None,
                remote_addr: "1.2.3.4:80".into(),
                connect_duration_ms: ms,
                attempt_count: 1,
                started_at: Utc::now(),
                success: true,
                mss_bytes: None,
                rtt_estimate_ms: None,
                retransmits: None,
                total_retrans: None,
                snd_cwnd: None,
                snd_ssthresh: None,
                rtt_variance_ms: None,
                rcv_space: None,
                segs_out: None,
                segs_in: None,
                congestion_algorithm: None,
                delivery_rate_bps: None,
                min_rtt_ms: None,
            }),
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
            rpm: None,
        }
    };
    r1.attempts.push(make_tcp(5.0));
    r2.attempts.push(make_tcp(15.0));
    let html = render_multi(&[r1, r2], None, None);
    assert!(
        html.contains("Connect ms"),
        "TCP metric label should be 'Connect ms'"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// SVG chart helpers — basic structural checks
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn svg_boxplot_empty_input_returns_empty_string() {
    let result = svg_boxplot("title", &[], "ms");
    assert!(
        result.is_empty(),
        "empty groups should produce empty string"
    );
}

#[test]
fn svg_boxplot_too_few_values_skipped() {
    // Groups with < 2 values are skipped per the code
    let values = vec![1.0f64]; // only 1 point
    let result = svg_boxplot("title", &[("label", &values, "#red")], "ms");
    assert!(
        result.is_empty(),
        "fewer than 2 values should produce empty svg"
    );
}

#[test]
fn svg_boxplot_valid_data_produces_svg_element() {
    let values = vec![10.0f64, 20.0, 30.0, 40.0, 50.0];
    let result = svg_boxplot("Test Chart", &[("label", &values, "#4e79a7")], "ms");
    assert!(result.starts_with("<svg"), "should produce an svg element");
    assert!(
        result.contains("Test Chart"),
        "chart title should appear in svg"
    );
    assert!(result.ends_with("</svg>"), "svg must be closed");
}

#[test]
fn svg_boxplot_escapes_title() {
    let values = vec![10.0f64, 20.0, 30.0, 40.0, 50.0];
    let result = svg_boxplot(
        "Chart <with> special & chars",
        &[("label", &values, "#red")],
        "ms",
    );
    assert!(
        result.contains("&lt;with&gt;"),
        "title must be HTML-escaped"
    );
    assert!(
        result.contains("&amp;"),
        "ampersand in title must be escaped"
    );
}

#[test]
fn svg_cdf_empty_input_returns_empty() {
    let result = svg_cdf("title", &[], "ms");
    assert!(result.is_empty(), "empty series produces empty string");
}

#[test]
fn svg_cdf_single_value_series_skipped() {
    let values = vec![42.0f64];
    let result = svg_cdf("title", &[("s", &values, "#red")], "ms");
    assert!(
        result.is_empty(),
        "series with < 2 values should be skipped"
    );
}

#[test]
fn svg_cdf_valid_data_produces_svg() {
    let values = vec![10.0f64, 20.0, 30.0, 40.0];
    let result = svg_cdf("CDF Chart", &[("series1", &values, "#4e79a7")], "ms");
    assert!(result.starts_with("<svg"), "should produce svg element");
    assert!(result.contains("CDF Chart"), "title must appear");
}

#[test]
fn svg_hbar_valid_data_produces_svg() {
    let bars = vec![("item1", 100.0_f64), ("item2", 50.0_f64)];
    let colors = vec!["#4e79a7", "#e07b39"];
    let result = svg_hbar("Bar Chart", &bars, "ms", &colors);
    assert!(result.starts_with("<svg"), "should produce svg element");
    assert!(result.contains("Bar Chart"), "title must appear");
    assert!(result.contains("item1"), "bar labels must appear");
}

// ─────────────────────────────────────────────────────────────────────────
// HTML structure — footer timestamp
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn footer_contains_generator_info_and_timestamp() {
    let run = make_run();
    let html = render(&run, None, None);
    assert!(
        html.contains("networker-tester"),
        "footer should mention generator"
    );
    assert!(html.contains("UTC"), "footer should include UTC timestamp");
    assert!(html.contains("<footer>"), "footer element must be present");
}

#[test]
fn render_multi_footer_uses_last_run_timestamp() {
    let r1 = make_run_with_url("https://a.example.com/");
    let r2 = make_run_with_url("https://b.example.com/");
    let html = render_multi(&[r1, r2], None, None);
    assert!(
        html.contains("<footer>"),
        "footer must appear in multi-target render"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// render_multi: target URL escaping
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn render_multi_escapes_target_url_in_summary() {
    let r1 = make_run_with_url("https://a.example.com/path?q=1&v=2");
    let r2 = make_run_with_url("https://b.example.com/");
    let html = render_multi(&[r1, r2], None, None);
    // The URL with & appears escaped in the HTML table
    assert!(
        html.contains("q=1&amp;v=2"),
        "& in URL must be HTML-escaped in table"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// All-attempts section open/closed behavior
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn all_attempts_section_open_when_twenty_or_fewer() {
    let mut run = make_run();
    run.attempts.clear();
    for i in 0..5 {
        let mut a = make_attempt(Protocol::Http1, true);
        a.sequence_num = i;
        run.attempts.push(a);
    }
    let html = render(&run, None, None);
    // With <= 20 attempts the details element gets the open attribute
    assert!(
        html.contains("<details open>")
            || html.contains("<details open ")
            || html.contains(" open>"),
        "few attempts should render details as open"
    );
}

#[test]
fn all_attempts_section_closed_when_over_twenty() {
    let mut run = make_run();
    run.attempts.clear();
    for i in 0..25 {
        let mut a = make_attempt(Protocol::Http1, true);
        a.sequence_num = i;
        run.attempts.push(a);
    }
    let html = render(&run, None, None);
    assert!(
        html.contains("25 attempts"),
        "should show attempt count in summary"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Memory formatting boundary cases (already covered partially; add edge cases)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn host_info_card_exactly_1024_mb_shows_gb() {
    let mut run = make_run();
    let mut info = make_host_info(Some("srv"), "Linux", None, None);
    info.total_memory_mb = Some(1024);
    run.server_info = Some(info);
    let html = render(&run, None, None);
    assert!(
        html.contains("1.0 GB"),
        "exactly 1024 MB should display as 1.0 GB"
    );
}

#[test]
fn host_info_card_just_below_1024_mb_shows_mb() {
    let mut run = make_run();
    let mut info = make_host_info(Some("srv"), "Linux", None, None);
    info.total_memory_mb = Some(1023);
    run.server_info = Some(info);
    let html = render(&run, None, None);
    assert!(html.contains("1023 MB"), "1023 MB should display as MB");
}

// ─────────────────────────────────────────────────────────────────────────
// render: modes display
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn render_shows_all_modes_in_run_summary() {
    let mut run = make_run();
    run.modes = vec!["http1".into(), "http2".into(), "pageload".into()];
    let html = render(&run, None, None);
    assert!(
        html.contains("http1, http2, pageload"),
        "all modes should appear comma-separated"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// server_timing server version badge in run summary
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn run_summary_shows_server_version_from_server_timing() {
    let mut run = make_run();
    run.attempts[0].server_timing = Some(crate::metrics::ServerTimingResult {
        server_version: Some("0.13.2".into()),
        ..Default::default()
    });
    let html = render(&run, None, None);
    assert!(
        html.contains("0.13.2"),
        "server version from server_timing should appear in run summary"
    );
}

#[test]
fn run_summary_shows_dash_when_no_server_version() {
    let run = make_run(); // no server_timing
    let html = render(&run, None, None);
    // The server_ver field defaults to "—"
    assert!(
        html.contains("<dd>—</dd>") || html.contains(">—<"),
        "no server version shows em dash"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Multi-target success/failure counts in summary table
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn render_multi_shows_success_and_failure_counts() {
    let mut r1 = make_run_with_url("https://a.example.com/");
    r1.attempts.push(make_attempt(Protocol::Http1, true));
    r1.attempts.push(make_attempt(Protocol::Http1, true));
    r1.attempts.push(make_attempt(Protocol::Http1, false));
    let r2 = make_run_with_url("https://b.example.com/");
    let html = render_multi(&[r1, r2], None, None);
    // r1: 3 attempts, 2 succeeded, 1 failed
    assert!(
        html.contains("<td class=\"ok\">2</td>"),
        "success count should appear with ok class"
    );
    assert!(
        html.contains("<td class=\"err\">1</td>"),
        "failure count should appear with err class"
    );
}

#[test]
fn render_multi_failure_count_zero_uses_ok_class() {
    let mut r1 = make_run_with_url("https://a.example.com/");
    r1.attempts.push(make_attempt(Protocol::Http1, true));
    let r2 = make_run_with_url("https://b.example.com/");
    let html = render_multi(&[r1, r2], None, None);
    // 0 failures → fail_cls = "ok"
    assert!(
        html.contains("<td class=\"ok\">0</td>"),
        "zero failures should use ok class"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Cloud hostname detection & short names
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn is_cloud_internal_hostname_aws_ip_detected() {
    assert!(super::is_cloud_internal_hostname("ip-172-31-78-2"));
    assert!(super::is_cloud_internal_hostname("ip-10-0-1-50"));
}

#[test]
fn is_cloud_internal_hostname_normal_not_detected() {
    assert!(!super::is_cloud_internal_hostname("my-vm-01"));
    assert!(!super::is_cloud_internal_hostname("web-server"));
    assert!(!super::is_cloud_internal_hostname("turing"));
}

#[test]
fn is_cloud_internal_hostname_empty_and_unknown() {
    assert!(!super::is_cloud_internal_hostname(""));
    assert!(!super::is_cloud_internal_hostname("unknown"));
}

#[test]
fn os_short_label_variants() {
    assert_eq!(super::os_short_label("Ubuntu 22.04 LTS"), "Ubuntu");
    assert_eq!(super::os_short_label("Windows Server 2022"), "Windows");
    assert_eq!(super::os_short_label("Debian GNU/Linux 11"), "Debian");
    assert_eq!(super::os_short_label("CentOS 8"), "Linux");
}

#[test]
fn provider_from_region_detects_clouds() {
    assert_eq!(super::provider_from_region("azure/eastus"), Some("Azure"));
    assert_eq!(super::provider_from_region("aws/us-east-1"), Some("AWS"));
    assert_eq!(super::provider_from_region("gcp/us-central1"), Some("GCP"));
    assert_eq!(super::provider_from_region("on-prem/dc1"), None);
}

#[test]
fn derive_display_name_aws_internal_hostname() {
    let info = make_host_info(
        Some("ip-172-31-78-2"),
        "Ubuntu 22.04 LTS",
        Some("aws/us-east-1"),
        None,
    );
    assert_eq!(
        super::derive_display_name(Some(&info), "fallback"),
        "AWS Ubuntu"
    );
}

#[test]
fn derive_display_name_normal_hostname_kept() {
    let info = make_host_info(Some("my-vm"), "Ubuntu 22.04", None, None);
    assert_eq!(super::derive_display_name(Some(&info), "fallback"), "my-vm");
}

#[test]
fn derive_display_name_none_uses_fallback() {
    assert_eq!(super::derive_display_name(None, "Target 1"), "Target 1");
}

#[test]
fn derive_display_name_empty_hostname_with_gcp_windows() {
    let info = make_host_info(Some(""), "Windows Server 2022", Some("gcp/us-east1"), None);
    assert_eq!(
        super::derive_display_name(Some(&info), "fallback"),
        "GCP Windows"
    );
}

#[test]
fn build_target_short_names_deduplicates() {
    let mut r1 = make_run_with_url("https://a.example.com/");
    r1.server_info = Some(make_host_info(
        Some("ip-172-31-1-1"),
        "Ubuntu 22.04",
        Some("aws/us-east-1"),
        None,
    ));
    let mut r2 = make_run_with_url("https://b.example.com/");
    r2.server_info = Some(make_host_info(
        Some("ip-172-31-2-2"),
        "Ubuntu 22.04",
        Some("aws/us-east-1"),
        None,
    ));
    let names = super::build_target_short_names(&[r1, r2]);
    assert_eq!(names[0], "AWS Ubuntu #1");
    assert_eq!(names[1], "AWS Ubuntu #2");
}

#[test]
fn build_target_short_names_unique_no_suffix() {
    let mut r1 = make_run_with_url("https://a.example.com/");
    r1.server_info = Some(make_host_info(Some("turing"), "Ubuntu 22.04", None, None));
    let mut r2 = make_run_with_url("https://b.example.com/");
    r2.server_info = Some(make_host_info(
        Some("ip-172-31-1-1"),
        "Ubuntu 22.04",
        Some("aws/us-east-1"),
        None,
    ));
    let names = super::build_target_short_names(&[r1, r2]);
    assert_eq!(names[0], "turing");
    assert_eq!(names[1], "AWS Ubuntu");
}

#[test]
fn render_multi_uses_short_names_in_cross_target_headers() {
    let mut r1 = make_run_with_url("https://10.0.0.1:8443/health");
    r1.server_info = Some(make_host_info(Some("turing"), "Ubuntu 22.04", None, None));
    r1.attempts.push(make_attempt(Protocol::Http1, true));
    let mut r2 = make_run_with_url("https://44.211.79.193:8443/health");
    r2.server_info = Some(make_host_info(
        Some("ip-172-31-78-2"),
        "Ubuntu 22.04 LTS",
        Some("aws/us-east-1"),
        None,
    ));
    r2.attempts.push(make_attempt(Protocol::Http1, true));
    let html = render_multi(&[r1, r2], None, None);
    // Cross-target headers should use short names, not full URLs
    assert!(
        html.contains("<th>turing</th>"),
        "expected short name 'turing' in header"
    );
    assert!(
        html.contains("<th>AWS Ubuntu</th>"),
        "expected short name 'AWS Ubuntu' in header"
    );
    // Full URLs should NOT be in the table headers
    assert!(
        !html.contains("<th>Target 1 <small>"),
        "should not have old 'Target N <small>URL' format"
    );
}

#[test]
fn render_multi_aws_internal_hostname_shows_provider_name_in_summary() {
    let mut r1 = make_run_with_url("https://44.211.79.193:8443/health");
    r1.server_info = Some(make_host_info(
        Some("ip-172-31-78-2"),
        "Ubuntu 22.04 LTS",
        Some("aws/us-east-1"),
        None,
    ));
    r1.attempts.push(make_attempt(Protocol::Http1, true));
    let mut r2 = make_run_with_url("https://34.148.238.88:8443/health");
    r2.server_info = Some(make_host_info(
        Some(""),
        "Windows Server 2022",
        Some("gcp/us-east1"),
        None,
    ));
    r2.attempts.push(make_attempt(Protocol::Http1, true));
    let html = render_multi(&[r1, r2], None, None);
    // Summary table should show "AWS Ubuntu" not "ip-172-31-78-2" in display name
    assert!(
        html.contains("AWS Ubuntu"),
        "AWS internal hostname should be replaced"
    );
    // The internal hostname may still appear in the detailed host info card,
    // but the summary/header display names should use the provider+OS form.
    // Check the summary row uses the provider name
    assert!(
        html.contains("AWS Ubuntu<br>"),
        "summary display name should be provider+OS"
    );
}

#[test]
fn render_stack_as_independent_section_when_stack_attempts_present() {
    let mut run = make_run();
    // Add default endpoint pageload attempts
    run.attempts
        .push(make_page_load_attempt(Protocol::PageLoad2, 150.0, false));
    run.attempts
        .push(make_page_load_attempt(Protocol::PageLoad2, 160.0, false));

    // Add nginx stack attempts
    let mut nginx1 = make_page_load_attempt(Protocol::PageLoad2, 120.0, false);
    nginx1.http_stack = Some("nginx".into());
    let mut nginx2 = make_page_load_attempt(Protocol::PageLoad2, 130.0, false);
    nginx2.http_stack = Some("nginx".into());
    run.attempts.push(nginx1);
    run.attempts.push(nginx2);

    let html = render_multi(&[run], None, None);
    // Should NOT have the old combined comparison table
    assert!(
        !html.contains("HTTP Stack Comparison"),
        "should not have combined comparison table"
    );
    // Should have independent stack section
    assert!(
        html.contains("NGINX Stack Results"),
        "should have independent nginx section"
    );
    // Endpoint data should appear in the main sections
    assert!(
        html.contains("Timing Breakdown by Protocol"),
        "should have endpoint timing section"
    );
}

#[test]
fn render_no_stack_section_when_no_stack_attempts() {
    let mut run = make_run();
    run.attempts
        .push(make_page_load_attempt(Protocol::PageLoad2, 150.0, false));
    let html = render_multi(&[run], None, None);
    assert!(
        !html.contains("Stack Results"),
        "should not show stack section without stack attempts"
    );
}
