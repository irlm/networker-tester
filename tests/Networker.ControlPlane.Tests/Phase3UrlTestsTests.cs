using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Ports the Rust <c>db::url_tests</c> unit tests: CSV / multiline splitting and
/// the <c>section_detail</c> folding + derived origin / connection summaries.
/// </summary>
public class Phase3UrlTestsTests
{
    [Fact]
    public void SplitCsvList_ignores_empty_entries()
    {
        Assert.Equal(new[] { "h1", "h2", "h3" }, UrlTestsEndpoints.SplitCsvList("h1, h2,,h3 "));
    }

    [Fact]
    public void SplitMultilineList_ignores_blank_lines()
    {
        Assert.Equal(new[] { "first", "second" }, UrlTestsEndpoints.SplitMultilineList("first\n\n second \n"));
    }

    [Fact]
    public void SectionDetail_groups_fields_and_derives_summaries()
    {
        var detail = new UrlTestDetail
        {
            id = Guid.NewGuid(),
            started_at = DateTime.UtcNow,
            completed_at = null,
            requested_url = "https://example.com",
            final_url = "https://www.example.com",
            status = "completed",
            page_load_strategy = "browser",
            browser_engine = "chromium",
            browser_version = "123.0",
            primary_origin = "https://www.example.com",
            observed_protocol_primary_load = "h3",
            advertised_alt_svc = "h3=\":443\"",
            validated_http_versions = new List<string> { "h2", "h3" },
            tls_version = "TLS 1.3",
            cipher_suite = "TLS_AES_128_GCM_SHA256",
            alpn = "h3",
            total_requests = 4,
            total_transfer_bytes = 4096,
            peak_concurrent_connections = 3,
            redirect_count = 1,
            failure_count = 0,
            har_path = "/tmp/url.har",
            pcap_summary = new UrlPacketCaptureSummaryView
            {
                mode = "tester",
                @interface = "lo",
                capture_path = "/tmp/packet-capture.pcapng",
                total_packets = 42,
                capture_status = "captured",
            },
            capture_errors = new List<string> { "pcap unavailable" },
            environment_notes = "linux runner",
            resources = new List<UrlTestResourceRow>
            {
                new()
                {
                    resource_url = "https://www.example.com/app.js",
                    origin = "https://www.example.com",
                    resource_type = "script",
                    status_code = 200,
                    protocol = "h3",
                    transfer_size = 2048,
                    encoded_body_size = 1800,
                    decoded_body_size = 4096,
                    duration_ms = 12.0,
                    from_cache = false,
                    redirected = false,
                    failed = false,
                },
            },
            protocol_runs = new List<UrlTestProtocolRunRow>
            {
                new()
                {
                    protocol_mode = "h3",
                    run_number = 1,
                    attempt_type = "probe",
                    observed_protocol = "h3",
                    fallback_occurred = false,
                    succeeded = true,
                    status_code = 200,
                    ttfb_ms = 55.0,
                    total_ms = 300.0,
                },
            },
        };

        var sectioned = UrlTestsEndpoints.SectionDetail(detail);

        Assert.Equal("completed", sectioned.overview.status);
        Assert.Equal(new[] { "h2", "h3" }, sectioned.protocol.validated_http_versions);
        Assert.Equal("h3", sectioned.tls.alpn);
        Assert.Equal(new[] { "pcap unavailable" }, sectioned.artifacts.capture_errors);
        Assert.Equal(42UL, sectioned.artifacts.pcap_summary!.total_packets);
        // capture_path is redacted to the file name.
        Assert.Equal("packet-capture.pcapng", sectioned.artifacts.pcap_summary.capture_path);
        // har_path redacted to file name (it was already a raw path in this DTO).
        Assert.Single(sectioned.origin_summaries);
        Assert.Equal(1u, sectioned.connection_summary!.peak_origin_request_count);
        Assert.Single(sectioned.resources);
        Assert.Single(sectioned.protocol_runs);
    }

    [Fact]
    public void SummarizeOrigins_ranks_protocols_by_count_then_name()
    {
        var resources = new List<UrlTestResourceRow>
        {
            new() { origin = "o", protocol = "h2", failed = false },
            new() { origin = "o", protocol = "h2", failed = false },
            new() { origin = "o", protocol = "h3", failed = true },
        };

        var (origins, connection) = UrlTestsEndpoints.SummarizeOriginsAndConnections(resources);

        var summary = Assert.Single(origins);
        Assert.Equal(3u, summary.request_count);
        Assert.Equal(1u, summary.failure_count);
        Assert.Equal(new[] { "h2", "h3" }, summary.protocols); // h2 (2) before h3 (1)
        Assert.Equal("h2", summary.dominant_protocol);
        Assert.NotNull(connection);
    }
}
