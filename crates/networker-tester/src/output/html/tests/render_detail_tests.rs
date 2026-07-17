use super::*;

// ─────────────────────────────────────────────────────────────────────────
// render() — single-target output structure
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn render_single_target_has_html_structure() {
    let run = make_run();
    let html = render(&run, None, None);
    assert!(
        html.starts_with("<!DOCTYPE html>"),
        "must start with DOCTYPE"
    );
    assert!(html.contains("</html>"), "must close html tag");
    assert!(html.contains("<head>"), "must have head element");
    assert!(html.contains("<body>"), "must have body element");
    assert!(html.contains("</body>"), "must close body element");
}

#[test]
fn render_single_target_includes_run_summary() {
    let run = make_run();
    let html = render(&run, None, None);
    assert!(
        html.contains("Run Summary"),
        "should have Run Summary section"
    );
    assert!(
        html.contains("http://localhost/health"),
        "should show target URL"
    );
    assert!(html.contains("Succeeded"), "should show success field");
}

#[test]
fn render_with_no_finished_at_shows_dash_for_duration() {
    let mut run = make_run();
    run.finished_at = None;
    let html = render(&run, None, None);
    // The duration cell shows "—" when finished_at is None
    assert!(
        html.contains("—"),
        "missing finished_at should produce em dash for duration"
    );
}

#[test]
fn render_css_href_produces_link_element() {
    let run = make_run();
    let html = render(&run, Some("/static/report.css"), None);
    assert!(html.contains(r#"<link rel="stylesheet""#));
    assert!(html.contains("/static/report.css"));
}

#[test]
fn render_css_href_escapes_special_chars_in_path() {
    let run = make_run();
    let html = render(&run, Some("path/with&special<chars>"), None);
    // The href value is HTML-escaped
    assert!(html.contains("&amp;"), "& must be escaped in href");
}

#[test]
fn render_shows_network_baseline_when_present() {
    let mut run = make_run();
    run.baseline = Some(make_baseline(NetworkType::Internet, 42.5));
    let html = render(&run, None, None);
    assert!(
        html.contains("Network Baseline"),
        "should have Network Baseline card"
    );
    assert!(html.contains("42.50"), "should show RTT avg value");
    assert!(html.contains("Internet"), "should show network type");
}

#[test]
fn render_network_baseline_loopback_uses_ok_class() {
    let mut run = make_run();
    run.baseline = Some(make_baseline(NetworkType::Loopback, 0.1));
    let html = render(&run, None, None);
    // Loopback maps to "ok" CSS class
    assert!(html.contains("Loopback"), "should show Loopback label");
    // The net_cls for Loopback is "ok"
    assert!(
        html.contains(r#"<span class="ok">Loopback</span>"#),
        "loopback should use ok class"
    );
}

#[test]
fn render_network_baseline_lan_uses_warn_class() {
    let mut run = make_run();
    run.baseline = Some(make_baseline(NetworkType::LAN, 2.0));
    let html = render(&run, None, None);
    assert!(
        html.contains(r#"<span class="warn">LAN</span>"#),
        "LAN should use warn class"
    );
}

#[test]
fn render_network_baseline_internet_uses_err_class() {
    let mut run = make_run();
    run.baseline = Some(make_baseline(NetworkType::Internet, 50.0));
    let html = render(&run, None, None);
    assert!(
        html.contains(r#"<span class="err">Internet</span>"#),
        "Internet should use err class"
    );
}

#[test]
fn render_shows_client_info_card_when_present() {
    let mut run = make_run();
    run.client_info = Some(make_host_info(
        Some("client-host"),
        "Ubuntu 22.04",
        None,
        None,
    ));
    let html = render(&run, None, None);
    assert!(html.contains("Client Info"), "should have Client Info card");
    assert!(html.contains("client-host"), "should show client hostname");
}

#[test]
fn render_shows_server_info_card_when_present() {
    let mut run = make_run();
    run.server_info = Some(make_host_info(
        Some("server-host"),
        "Ubuntu 22.04",
        None,
        Some("0.13.2"),
    ));
    let html = render(&run, None, None);
    assert!(html.contains("Server Info"), "should have Server Info card");
    assert!(html.contains("server-host"), "should show server hostname");
    assert!(
        html.contains("Version"),
        "server card should show Version row"
    );
    assert!(html.contains("0.13.2"), "should show server version");
}

#[test]
fn render_server_info_shows_region_when_present() {
    let mut run = make_run();
    run.server_info = Some(make_host_info(
        Some("vm1"),
        "Ubuntu 22.04",
        Some("azure/eastus"),
        None,
    ));
    let html = render(&run, None, None);
    assert!(html.contains("Region"), "should have Region row");
    assert!(
        html.contains("azure/eastus"),
        "should show full region string"
    );
}

#[test]
fn render_server_info_no_region_row_when_absent() {
    let mut run = make_run();
    run.server_info = Some(make_host_info(Some("vm1"), "Ubuntu 22.04", None, None));
    let html = render(&run, None, None);
    // Region row only appears for the server card when region is set
    assert!(
        !html.contains("<dt>Region</dt>"),
        "no region row when region is absent"
    );
}

#[test]
fn render_server_info_uptime_appears_when_set() {
    let mut run = make_run();
    let mut info = make_host_info(Some("srv"), "Linux", None, None);
    info.uptime_secs = Some(3661); // 1h 1m 1s
    run.server_info = Some(info);
    let html = render(&run, None, None);
    assert!(html.contains("Uptime"), "should show Uptime row");
    assert!(
        html.contains("1h 1m"),
        "should format uptime as hours/minutes"
    );
}

#[test]
fn render_server_info_uptime_days_format() {
    let mut run = make_run();
    let mut info = make_host_info(Some("srv"), "Linux", None, None);
    info.uptime_secs = Some(86400 + 7200); // 1d 2h
    run.server_info = Some(info);
    let html = render(&run, None, None);
    assert!(
        html.contains("1d 2h"),
        "uptime >= 1 day should use day format"
    );
}

#[test]
fn render_server_info_uptime_minutes_format() {
    let mut run = make_run();
    let mut info = make_host_info(Some("srv"), "Linux", None, None);
    info.uptime_secs = Some(130); // 2m 10s
    run.server_info = Some(info);
    let html = render(&run, None, None);
    assert!(
        html.contains("2m 10s"),
        "uptime < 1h should use minute format"
    );
}

#[test]
fn render_shows_failed_attempts_in_err_class() {
    let mut run = make_run();
    run.attempts[0].success = false;
    run.attempts[0].http.as_mut().unwrap().status_code = 500;
    let html = render(&run, None, None);
    // Failed count should be > 0
    assert!(
        html.contains("row-err"),
        "failed attempt should have row-err class"
    );
}

#[test]
fn render_zero_failures_shows_ok_class_for_failed_count() {
    let run = make_run();
    let html = render(&run, None, None);
    // With 0 failures the fail_cls should be "ok"
    assert!(
        html.contains(r#"class="ok">0<"#) || html.contains("0</dd>"),
        "zero failures should display with ok class or zero value"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// write_host_info_card — memory display
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn host_info_card_memory_mb_display() {
    let mut run = make_run();
    let mut info = make_host_info(Some("srv"), "Linux", None, None);
    info.total_memory_mb = Some(512);
    run.server_info = Some(info);
    let html = render(&run, None, None);
    assert!(html.contains("512 MB"), "small memory should show in MB");
}

#[test]
fn host_info_card_memory_gb_display() {
    let mut run = make_run();
    let mut info = make_host_info(Some("srv"), "Linux", None, None);
    info.total_memory_mb = Some(8192); // 8 GiB
    run.server_info = Some(info);
    let html = render(&run, None, None);
    assert!(html.contains("8.0 GB"), "large memory should show in GB");
}

#[test]
fn host_info_card_no_memory_shows_dash() {
    let mut run = make_run();
    let mut info = make_host_info(Some("srv"), "Linux", None, None);
    info.total_memory_mb = None;
    run.server_info = Some(info);
    let html = render(&run, None, None);
    // The memory line shows "—" when total_memory_mb is None
    assert!(
        html.contains("<dd>—</dd>"),
        "absent memory should show em dash"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// render_multi() — multi-target output structure
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn render_multi_single_run_delegates_to_render() {
    let run = make_run();
    let multi = render_multi(std::slice::from_ref(&run), None, None);
    let single = render(&run, None, None);
    // Single-run multi should produce identical output to render()
    assert_eq!(multi, single, "single-run render_multi must equal render");
}

#[test]
fn render_multi_two_targets_shows_summary_table() {
    let r1 = make_run_with_url("https://target1.example.com/");
    let r2 = make_run_with_url("https://target2.example.com/");
    let html = render_multi(&[r1, r2], None, None);
    assert!(
        html.contains("Multi-Target Summary"),
        "must have summary table"
    );
    assert!(
        html.contains("target1.example.com"),
        "must show first target"
    );
    assert!(
        html.contains("target2.example.com"),
        "must show second target"
    );
}

#[test]
fn render_multi_two_targets_shows_comparison_section() {
    let mut r1 = make_run_with_url("https://target1.example.com/");
    let mut r2 = make_run_with_url("https://target2.example.com/");
    // Add HTTP/1 attempts so the protocol comparison table appears
    r1.attempts.push(make_attempt(Protocol::Http1, true));
    r2.attempts.push(make_attempt(Protocol::Http1, true));
    let html = render_multi(&[r1, r2], None, None);
    assert!(
        html.contains("Cross-Target Protocol Comparison"),
        "must have comparison table"
    );
}

#[test]
fn render_multi_shows_target_count_in_title() {
    let r1 = make_run_with_url("https://a.example.com/");
    let r2 = make_run_with_url("https://b.example.com/");
    let r3 = make_run_with_url("https://c.example.com/");
    let html = render_multi(&[r1, r2, r3], None, None);
    assert!(html.contains("3 targets compared"), "title must show count");
}

#[test]
fn render_multi_per_target_details_have_open_attr_for_two_targets() {
    let r1 = make_run_with_url("https://a.example.com/");
    let r2 = make_run_with_url("https://b.example.com/");
    let html = render_multi(&[r1, r2], None, None);
    // For <= 2 runs each details element should be open
    assert!(
        html.contains("<details class=\"card multi-target-details\" open>"),
        "2-target runs should have open details"
    );
}

#[test]
fn render_multi_three_targets_details_closed_by_default() {
    let r1 = make_run_with_url("https://a.example.com/");
    let r2 = make_run_with_url("https://b.example.com/");
    let r3 = make_run_with_url("https://c.example.com/");
    let html = render_multi(&[r1, r2, r3], None, None);
    // With 3 targets the details should NOT have the open attribute
    assert!(
        html.contains("<details class=\"card multi-target-details\">"),
        "3-target runs should have closed details by default"
    );
    assert!(
        !html.contains("<details class=\"card multi-target-details\" open>"),
        "3-target runs must not have open attribute"
    );
}

#[test]
fn render_multi_target_baseline_rtt_shown_in_summary() {
    let mut r1 = make_run_with_url("https://a.example.com/");
    r1.baseline = Some(make_baseline(NetworkType::Internet, 30.5));
    let mut r2 = make_run_with_url("https://b.example.com/");
    r2.baseline = Some(make_baseline(NetworkType::Internet, 80.2));
    let html = render_multi(&[r1, r2], None, None);
    assert!(html.contains("30.50"), "should show r1 RTT avg");
    assert!(html.contains("80.20"), "should show r2 RTT avg");
}

#[test]
fn render_multi_target_duration_shown_when_finished_at_set() {
    let mut r1 = make_run_with_url("https://a.example.com/");
    let now = Utc::now();
    r1.started_at = now;
    r1.finished_at = Some(now + chrono::Duration::milliseconds(2500));
    let r2 = make_run_with_url("https://b.example.com/");
    let html = render_multi(&[r1, r2], None, None);
    assert!(html.contains("2.50s"), "should show formatted duration");
}

// ─────────────────────────────────────────────────────────────────────────
// Server name display logic in render_multi summary table
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn server_name_uses_hostname_when_present() {
    let mut run = make_run_with_url("https://target.example.com/");
    run.server_info = Some(make_host_info(Some("my-vm-01"), "Ubuntu 22.04", None, None));
    let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
    assert!(
        html.contains("my-vm-01"),
        "hostname should be used as display name"
    );
}

#[test]
fn server_name_unknown_hostname_falls_back_to_provider_os() {
    let mut run = make_run_with_url("https://target.example.com/");
    run.server_info = Some(make_host_info(
        Some("unknown"),
        "Ubuntu 22.04 LTS",
        Some("azure/eastus"),
        None,
    ));
    let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
    // Should show "Azure Ubuntu" derived from region prefix and OS
    assert!(
        html.contains("Azure Ubuntu"),
        "unknown hostname should yield provider+OS name"
    );
}

#[test]
fn server_name_empty_hostname_falls_back_to_provider_os() {
    let mut run = make_run_with_url("https://target.example.com/");
    run.server_info = Some(make_host_info(
        Some(""),
        "Windows Server 2022",
        Some("aws/us-east-1"),
        None,
    ));
    let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
    assert!(
        html.contains("AWS Windows"),
        "empty hostname should yield provider+OS name"
    );
}

#[test]
fn server_name_no_server_info_shows_dash() {
    let run1 = make_run_with_url("https://target.example.com/");
    let run2 = make_run_with_url("https://b.com/");
    let html = render_multi(&[run1, run2], None, None);
    // No server_info → "—" in Server column
    assert!(
        html.contains("<td>—</td>"),
        "no server info should show em dash"
    );
}

#[test]
fn server_name_gcp_region_detected() {
    let mut run = make_run_with_url("https://target.example.com/");
    run.server_info = Some(make_host_info(
        Some("unknown"),
        "Ubuntu 22.04",
        Some("gcp/us-central1"),
        None,
    ));
    let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
    assert!(
        html.contains("GCP Ubuntu"),
        "gcp/ prefix should map to GCP provider"
    );
}

#[test]
fn server_name_no_provider_region_falls_back_to_os_type() {
    let mut run = make_run_with_url("https://target.example.com/");
    run.server_info = Some(make_host_info(Some(""), "Ubuntu 20.04", None, None));
    let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
    // No region → no provider → just "Ubuntu"
    assert!(
        html.contains(">Ubuntu<") || html.contains(">Ubuntu "),
        "no provider gives just OS type"
    );
}

#[test]
fn server_name_windows_os_type_detected() {
    let mut run = make_run_with_url("https://target.example.com/");
    run.server_info = Some(make_host_info(Some(""), "Windows Server 2022", None, None));
    let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
    assert!(html.contains("Windows"), "Windows OS should be detected");
}

#[test]
fn server_name_generic_linux_os_type() {
    let mut run = make_run_with_url("https://target.example.com/");
    run.server_info = Some(make_host_info(Some(""), "Debian GNU/Linux 11", None, None));
    let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
    // Not Windows, not Ubuntu → falls back to "Linux"
    assert!(
        html.contains("Linux"),
        "unknown distro should fall back to Linux"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Version badge rendering (server_version field)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn version_badge_appears_in_multi_summary_when_set() {
    let mut run = make_run_with_url("https://target.example.com/");
    run.server_info = Some(make_host_info(
        Some("my-vm"),
        "Ubuntu 22.04",
        None,
        Some("0.13.2"),
    ));
    let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
    assert!(
        html.contains("<code>v0.13.2</code>"),
        "version badge must appear in summary"
    );
}

#[test]
fn version_badge_absent_when_server_version_none() {
    let mut run = make_run_with_url("https://target.example.com/");
    run.server_info = Some(make_host_info(Some("my-vm"), "Ubuntu 22.04", None, None));
    let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
    // No server_version → no <code>v...
    assert!(
        !html.contains("<code>v"),
        "no version badge when server_version is None"
    );
}

#[test]
fn version_badge_in_host_info_card_shows_version() {
    let mut run = make_run();
    run.server_info = Some(make_host_info(Some("srv"), "Linux", None, Some("1.2.3")));
    let html = render(&run, None, None);
    // In the Server Info card the version row shows the version string
    assert!(
        html.contains("1.2.3"),
        "server version must appear in single-target render"
    );
}

#[test]
fn version_badge_dash_when_no_server_version_in_host_card() {
    let mut run = make_run();
    run.server_info = Some(make_host_info(Some("srv"), "Linux", None, None));
    let html = render(&run, None, None);
    // The version row shows "—" when server_version is None
    assert!(
        html.contains("—"),
        "absent server_version should show em dash in card"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Region display in server summary
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn region_shown_in_multi_summary_table_when_set() {
    let mut run = make_run_with_url("https://target.example.com/");
    run.server_info = Some(make_host_info(
        Some("vm"),
        "Ubuntu 22.04",
        Some("azure/westeurope"),
        None,
    ));
    let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
    assert!(
        html.contains("azure/westeurope"),
        "region should appear in summary table"
    );
}

#[test]
fn region_absent_no_region_row_in_summary() {
    let mut run = make_run_with_url("https://target.example.com/");
    run.server_info = Some(make_host_info(Some("vm"), "Ubuntu 22.04", None, None));
    let html = render_multi(&[run, make_run_with_url("https://b.com/")], None, None);
    // The region small element only appears when region is Some
    assert!(
        !html.contains("Region: "),
        "no region marker when region is absent"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// LAN/Loopback detection and dimmed reference rendering
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn lan_target_shows_warn_badge_in_summary() {
    let mut r1 = make_run_with_url("https://a.example.com/");
    r1.baseline = Some(make_baseline(NetworkType::LAN, 1.0));
    let r2 = make_run_with_url("https://b.example.com/");
    let html = render_multi(&[r1, r2], None, None);
    assert!(
        html.contains(r#"<span class="warn">LAN</span>"#),
        "LAN network type should use warn class in summary"
    );
}

#[test]
fn loopback_target_shows_ok_badge_in_summary() {
    let mut r1 = make_run_with_url("http://localhost/");
    r1.baseline = Some(make_baseline(NetworkType::Loopback, 0.05));
    let r2 = make_run_with_url("https://b.example.com/");
    let html = render_multi(&[r1, r2], None, None);
    assert!(
        html.contains(r#"<span class="ok">Loopback</span>"#),
        "Loopback network type should use ok class in summary"
    );
}

#[test]
fn lan_target_shows_dimmed_ref_in_protocol_comparison() {
    let mut r_lan = make_run_with_url("http://192.168.1.100/");
    r_lan.baseline = Some(make_baseline(NetworkType::LAN, 0.5));
    let mut r_inet = make_run_with_url("https://remote.example.com/");
    r_inet.baseline = Some(make_baseline(NetworkType::Internet, 50.0));
    // Add Http1 attempts so the protocol comparison table shows data
    r_lan.attempts.push({
        let mut a = make_attempt(Protocol::Http1, true);
        a.http.as_mut().unwrap().total_duration_ms = 5.0;
        a
    });
    r_inet.attempts.push({
        let mut a = make_attempt(Protocol::Http1, true);
        a.http.as_mut().unwrap().total_duration_ms = 60.0;
        a
    });
    let html = render_multi(&[r_lan, r_inet], None, None);
    // LAN values should appear with opacity:.55 and (ref) label
    assert!(
        html.contains("opacity:.55"),
        "LAN target values should be dimmed"
    );
    assert!(html.contains("(ref)"), "LAN target should show (ref) label");
}

#[test]
fn internet_targets_show_diff_percentage_vs_baseline() {
    let mut r1 = make_run_with_url("https://fast.example.com/");
    r1.baseline = Some(make_baseline(NetworkType::Internet, 20.0));
    let mut r2 = make_run_with_url("https://slow.example.com/");
    r2.baseline = Some(make_baseline(NetworkType::Internet, 80.0));
    r1.attempts.push({
        let mut a = make_attempt(Protocol::Http1, true);
        a.http.as_mut().unwrap().total_duration_ms = 50.0;
        a
    });
    r2.attempts.push({
        let mut a = make_attempt(Protocol::Http1, true);
        a.http.as_mut().unwrap().total_duration_ms = 200.0;
        a
    });
    let html = render_multi(&[r1, r2], None, None);
    // One target is baseline (no diff), the other gets a <span class="diff-..."> element.
    // Check for span element usage specifically (CSS also defines these classes as plain names).
    let diff_span_count =
        html.matches(r#"class="diff-fast""#).count() + html.matches(r#"class="diff-slow""#).count();
    assert!(
        diff_span_count > 0,
        "comparison table must show diff percentage spans"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Baseline rank-sum selection — best Internet target becomes reference
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn rank_sum_baseline_selects_fastest_internet_target() {
    // Two Internet targets. Target 1 is consistently faster.
    // The comparison table should show Target 1 as baseline (raw values only),
    // and Target 2 with diff percentages.
    let mut r1 = make_run_with_url("https://fast.example.com/");
    r1.baseline = Some(make_baseline(NetworkType::Internet, 10.0));
    let mut r2 = make_run_with_url("https://slow.example.com/");
    r2.baseline = Some(make_baseline(NetworkType::Internet, 100.0));
    // Give both Http1 and Tcp attempts to build a real rank sum
    r1.attempts.push({
        let mut a = make_attempt(Protocol::Http1, true);
        a.http.as_mut().unwrap().total_duration_ms = 20.0;
        a
    });
    r2.attempts.push({
        let mut a = make_attempt(Protocol::Http1, true);
        a.http.as_mut().unwrap().total_duration_ms = 200.0;
        a
    });
    let html = render_multi(&[r1, r2], None, None);
    // When two Internet targets exist, actual <span class="diff-..."> elements appear.
    // The CSS defines these classes once each; actual usage in table cells adds more occurrences.
    // Count span elements specifically to distinguish CSS definitions from actual use.
    let diff_span_count =
        html.matches(r#"class="diff-fast""#).count() + html.matches(r#"class="diff-slow""#).count();
    assert!(
        diff_span_count > 0,
        "at least one diff span element must appear when two Internet targets exist"
    );
}

#[test]
fn rank_sum_with_single_internet_target_shows_no_diff() {
    // One Internet + one LAN. The LAN is reference; the Internet target has no Internet peer to diff against.
    let mut r_inet = make_run_with_url("https://remote.example.com/");
    r_inet.baseline = Some(make_baseline(NetworkType::Internet, 50.0));
    let mut r_lan = make_run_with_url("http://192.168.1.100/");
    r_lan.baseline = Some(make_baseline(NetworkType::LAN, 1.0));
    r_inet.attempts.push({
        let mut a = make_attempt(Protocol::Http1, true);
        a.http.as_mut().unwrap().total_duration_ms = 100.0;
        a
    });
    r_lan.attempts.push({
        let mut a = make_attempt(Protocol::Http1, true);
        a.http.as_mut().unwrap().total_duration_ms = 5.0;
        a
    });
    let html = render_multi(&[r_inet, r_lan], None, None);
    // With only one Internet target it is its own baseline — no <span class="diff-..."> elements
    // (CSS still defines .diff-fast and .diff-slow as plain class names, so we look for span elements)
    let diff_span_count =
        html.matches(r#"class="diff-fast""#).count() + html.matches(r#"class="diff-slow""#).count();
    assert!(
            diff_span_count == 0,
            "single internet target with one LAN should not produce diff span elements, got {diff_span_count}"
        );
}

// ─────────────────────────────────────────────────────────────────────────
// Cross-target observations text
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn cross_target_observations_mentions_fastest_protocol() {
    let mut r1 = make_run_with_url("https://fast.example.com/");
    r1.baseline = Some(make_baseline(NetworkType::Internet, 20.0));
    let mut r2 = make_run_with_url("https://slow.example.com/");
    r2.baseline = Some(make_baseline(NetworkType::Internet, 80.0));
    r1.attempts.push({
        let mut a = make_attempt(Protocol::Http1, true);
        a.http.as_mut().unwrap().total_duration_ms = 25.0;
        a
    });
    r2.attempts.push({
        let mut a = make_attempt(Protocol::Http1, true);
        a.http.as_mut().unwrap().total_duration_ms = 95.0;
        a
    });
    let html = render_multi(&[r1, r2], None, None);
    assert!(
        html.contains("fastest") || html.contains("faster"),
        "cross-target observations should mention fastest target"
    );
}

#[test]
fn cross_target_rtt_observation_appears_when_rtts_differ() {
    let mut r1 = make_run_with_url("https://a.example.com/");
    r1.baseline = Some(make_baseline(NetworkType::Internet, 5.0));
    let mut r2 = make_run_with_url("https://b.example.com/");
    r2.baseline = Some(make_baseline(NetworkType::Internet, 100.0));
    // Need at least one protocol row to trigger the observations block
    r1.attempts.push({
        let mut a = make_attempt(Protocol::Http1, true);
        a.http.as_mut().unwrap().total_duration_ms = 20.0;
        a
    });
    r2.attempts.push({
        let mut a = make_attempt(Protocol::Http1, true);
        a.http.as_mut().unwrap().total_duration_ms = 200.0;
        a
    });
    let html = render_multi(&[r1, r2], None, None);
    assert!(
        html.contains("Baseline RTT"),
        "RTT observation should appear when RTTs differ"
    );
}

#[test]
fn cross_target_mixed_network_observation_appears() {
    let mut r1 = make_run_with_url("https://a.example.com/");
    r1.baseline = Some(make_baseline(NetworkType::Internet, 50.0));
    let mut r2 = make_run_with_url("http://192.168.1.1/");
    r2.baseline = Some(make_baseline(NetworkType::LAN, 1.0));
    r1.attempts.push({
        let mut a = make_attempt(Protocol::Http1, true);
        a.http.as_mut().unwrap().total_duration_ms = 100.0;
        a
    });
    r2.attempts.push({
        let mut a = make_attempt(Protocol::Http1, true);
        a.http.as_mut().unwrap().total_duration_ms = 10.0;
        a
    });
    let html = render_multi(&[r1, r2], None, None);
    assert!(
        html.contains("Mixed network types"),
        "should note mixed network types"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Network type badge rendering in summary table
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn summary_table_dash_when_no_baseline() {
    let r1 = make_run_with_url("https://a.example.com/");
    let r2 = make_run_with_url("https://b.example.com/");
    let html = render_multi(&[r1, r2], None, None);
    // Without a baseline the Network column shows "—"
    assert!(
        html.contains("<td>—</td>"),
        "no baseline should show em dash for Network type"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Throughput protocol comparison in multi-target table
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn throughput_protocol_comparison_higher_is_better() {
    // For throughput protocols, the target with higher MB/s is "faster".
    let mut r1 = make_run_with_url("https://a.example.com/");
    r1.baseline = Some(make_baseline(NetworkType::Internet, 20.0));
    let mut r2 = make_run_with_url("https://b.example.com/");
    r2.baseline = Some(make_baseline(NetworkType::Internet, 20.0));

    let make_dl = |mbps: f64| -> RequestAttempt {
        let run_id = Uuid::new_v4();
        RequestAttempt {
            attempt_id: Uuid::new_v4(),
            run_id,
            protocol: Protocol::Download,
            sequence_num: 0,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
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
                total_duration_ms: 100.0,
                redirect_count: 0,
                started_at: Utc::now(),
                response_headers: vec![],
                payload_bytes: 1_048_576,
                throughput_mbps: Some(mbps),
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
            http_stack: None,
        }
    };
    // r1 has 200 MB/s (better), r2 has 100 MB/s (worse)
    r1.attempts.push(make_dl(200.0));
    r2.attempts.push(make_dl(100.0));
    let html = render_multi(&[r1, r2], None, None);
    assert!(
        html.contains("Throughput MB/s"),
        "Download metric label must appear"
    );
    // r1 (200 MB/s) is the baseline — raw value shown.
    // r2 (100 MB/s) is worse → gets class="diff-slow" (negative throughput delta).
    // Look for span elements specifically (not just the CSS class definition).
    assert!(
        html.contains(r#"class="diff-slow""#),
        "lower-throughput target should produce a diff-slow span element"
    );
}
