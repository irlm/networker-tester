using Networker.ControlPlane.Endpoints;
using Networker.ControlPlane.Infra;
using Networker.ControlPlane.Reports;
using Networker.ControlPlane.Reports.Documents;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins the report-side port of the dashboard's run-page analytics
/// (v0.28.128): per-payload latency rows, success-only stats, log-scale
/// candles, probe-duration modes out of the latency chart, MB/s units, the
/// throughput section, and the infrastructure envelope (assessment + advisor
/// mirroring lib/infra.ts + lib/advisor.ts — the real B2s scenario numbers).
/// The exported PDF/HTML/MD/DOCX previously showed none of this (user report,
/// test-run-ebd1a684.pdf).
/// </summary>
public sealed class RunReportParityTests
{
    private static AttemptView Throughput(
        string protocol, long payload, double mbs, int seq = 0, bool success = true) => new(
        AttemptId: Guid.NewGuid(), Protocol: protocol, SequenceNum: seq,
        StartedAt: DateTime.UtcNow, FinishedAt: DateTime.UtcNow.AddMilliseconds(100),
        Success: success, ErrorMessage: null, RetryCount: 0,
        Http: new AttemptHttpView(200, "HTTP/1.1", 1, 100, 0, 0, payload, mbs));

    private static AttemptView Timeout(string protocol, double wallMs, int seq = 0) => new(
        AttemptId: Guid.NewGuid(), Protocol: protocol, SequenceNum: seq,
        StartedAt: DateTime.UtcNow, FinishedAt: DateTime.UtcNow.AddMilliseconds(wallMs),
        Success: false, ErrorMessage: "timed out", RetryCount: 0);

    private static AttemptView Mthroughput(double capDown, double capUp) => new(
        AttemptId: Guid.NewGuid(), Protocol: "mthroughput", SequenceNum: 99,
        StartedAt: DateTime.UtcNow, FinishedAt: DateTime.UtcNow.AddSeconds(16),
        Success: true, ErrorMessage: null, RetryCount: 0,
        Mthroughput: new AttemptMthroughputView(capDown, capUp, 4, 3, null, null));

    private static RunReportInput Input(
        IReadOnlyList<AttemptView> attempts, ReportRunInfra? infra = null) => new(
        Guid.NewGuid(), "proj", "cfg", "target", "completed",
        DateTime.UtcNow.AddMinutes(-10), DateTime.UtcNow,
        attempts.Count(a => a.Success), attempts.Count(a => !a.Success), null,
        attempts, infra);

    private static ReportRunInfra B2sInfra() {
        var spec = VmNetworkSpecs.Lookup("azure", "Standard_B2s")!;
        var alts = new[]
        {
            new ReportAltSize("Standard_F2s_v2", 875, "estimated", true, 0.085),
            new ReportAltSize("Standard_D2s_v3", 1000, "estimated", true, 0.096),
            new ReportAltSize("Standard_D4s_v5", 12500, "documented", true, 0.192),
        };
        var side = new ReportInfraSide("azure", "Standard_B2s", "eastus", spec, 0.0416, alts);
        return new ReportRunInfra(side, side, PeakLoad1m: 0.21, CpuCores: 2);
    }

    private static ReportSection Section(ReportDocument doc, string title) =>
        Assert.Single(doc.Sections, s => s.Heading == title);

    // ── latency section parity ──────────────────────────────────────────────

    [Fact]
    public void Throughput_modes_get_one_latency_row_per_payload()
    {
        var doc = RunReportDocument.Build(Input(new[]
        {
            Throughput("download", 1024, 20, seq: 1),
            Throughput("download", 104857600, 73.2, seq: 2),
        }));
        var table = Section(doc, "Latency by protocol").Blocks.OfType<TableBlock>().First();
        var labels = table.Rows.Select(r => r[0]).ToList();
        Assert.Contains("DOWNLOAD · 1 KB", labels);
        Assert.Contains("DOWNLOAD · 100 MB", labels);
        Assert.DoesNotContain("DOWNLOAD", labels); // no merged all-payload row
    }

    [Fact]
    public void Latency_stats_are_success_only_and_the_candle_is_log_scaled()
    {
        // 10 fast successes + 10 × 50 s ping timeouts (the Azure-ICMP case):
        // the timeouts must not appear in the candle chart at all (no success
        // latency), and the chart must be log-scaled.
        var attempts = new List<AttemptView>();
        for (var i = 0; i < 10; i++)
        {
            attempts.Add(Throughput("http1", 0, 0, seq: i) with
            {
                Http = new AttemptHttpView(200, "HTTP/1.1", 1.2, 1.3, 0, 0, null, null),
            });
            attempts.Add(Timeout("ping", 50000, seq: 100 + i));
        }
        var doc = RunReportDocument.Build(Input(attempts));
        var candle = Section(doc, "Latency by protocol").Blocks.OfType<CandleBlock>().First();
        Assert.True(candle.LogScale);
        Assert.DoesNotContain(candle.Points, p => p.Label == "PING");
        // PING still appears in the table with Err=10 and dashed percentiles.
        var table = Section(doc, "Latency by protocol").Blocks.OfType<TableBlock>().First();
        var ping = table.Rows.First(r => r[0] == "PING");
        Assert.Equal("10", ping[3]);
        Assert.Equal("—", ping[4]);
    }

    [Fact]
    public void Probe_duration_modes_stay_out_of_the_latency_candle()
    {
        var doc = RunReportDocument.Build(Input(new[]
        {
            Throughput("http1", 0, 0) with
            {
                Http = new AttemptHttpView(200, "HTTP/1.1", 1.2, 1.3, 0, 0, null, null),
            },
            Mthroughput(118.9, 115.4), // 16 s probe duration is not a latency
        }));
        var candle = Section(doc, "Latency by protocol").Blocks.OfType<CandleBlock>().First();
        Assert.DoesNotContain(candle.Points, p => p.Label.StartsWith("MTHROUGHPUT"));
    }

    [Fact]
    public void CandleBlock_log_fraction_spreads_decades_evenly()
    {
        var c = new CandleBlock(null, new[]
        {
            new CandlePoint("a", 1, null, 10, null, null, 1000),
        }, "ms", LogScale: true);
        Assert.Equal(0.0, c.Fraction(1), 3);
        Assert.Equal(1.0 / 3, c.Fraction(10), 3);   // one of three decades
        Assert.Equal(1.0, c.Fraction(1000), 3);
    }

    // ── throughput section + units ──────────────────────────────────────────

    [Fact]
    public void Throughput_section_reports_MBs_per_payload_and_units_are_fixed()
    {
        var doc = RunReportDocument.Build(Input(new[]
        {
            Throughput("upload", 104857600, 110.8, seq: 1),
            Throughput("upload", 1024, 0.7, seq: 2),
        }));
        var table = Section(doc, "Throughput").Blocks.OfType<TableBlock>().First();
        Assert.Equal(2, table.Rows.Count);
        Assert.Contains("MB/s", table.Rows[1][4]); // p50 unit
        // The attempts table detail also says MB/s now, never the old "Mbps".
        var attemptsTable = Section(doc, "Attempts").Blocks.OfType<TableBlock>().First();
        Assert.Contains(attemptsTable.Rows, r => r[4].Contains("MB/s"));
        Assert.DoesNotContain(attemptsTable.Rows, r => r[4].Contains(" Mbps"));
    }

    // ── infrastructure envelope (the real B2s numbers, pinned) ──────────────

    [Fact]
    public void Envelope_reads_network_bound_at_98pct_and_suggests_the_priced_upsize()
    {
        var attempts = new[] { Throughput("download", 104857600, 73.2) };
        var assessments = RunInfraAssessment.Assess(attempts, B2sInfra());
        var dl = Assert.Single(assessments);
        Assert.Equal("network-bound", dl.Verdict);
        Assert.Equal("target", dl.LimitingSide);
        Assert.True(dl.Utilization is > 0.9);

        var suggestions = RunInfraAssessment.Advise(assessments, B2sInfra());
        var up = Assert.Single(suggestions, s => s.Kind == "upsize");
        Assert.Contains("Standard_D2s_v3", up.Text);   // cheapest ≥1.5× ceiling
        Assert.Contains("+$0.054", up.Text);           // real hourly delta

        var doc = RunReportDocument.Build(Input(attempts, B2sInfra()));
        var section = Section(doc, "Infrastructure envelope");
        Assert.Contains(section.Blocks.OfType<CalloutBlock>(),
            b => b.Text.Contains("Standard_D2s_v3"));
        Assert.Contains(section.Blocks.OfType<CalloutBlock>(),
            b => b.Text.Contains("CPU stayed idle"));
    }

    [Fact]
    public void Measured_mthroughput_capacity_supersedes_the_estimate()
    {
        var attempts = new[]
        {
            Throughput("download", 104857600, 73.2),
            Mthroughput(118.9, 115.4), // measured path: 951 / 923 Mbps
        };
        var dl = RunInfraAssessment.Assess(attempts, B2sInfra())
            .Single(a => a.Direction == "download");
        Assert.Equal("measured", dl.Confidence);
        Assert.Null(dl.LimitingSide);                   // path fact, not one side's
        Assert.Equal(118.9 * 8, dl.ExpectedMbps!.Value, 3);
        // 586 / 951 ≈ 62% → headroom, matching the live dashboard verdict.
        Assert.Equal("headroom", dl.Verdict);
    }

    [Fact]
    public void No_infra_input_omits_the_envelope_section()
    {
        var doc = RunReportDocument.Build(Input(new[] { Throughput("download", 1024, 20) }));
        Assert.DoesNotContain(doc.Sections, s => s.Heading == "Infrastructure envelope");
    }

    // ── friendly target ─────────────────────────────────────────────────────

    [Fact]
    public void Friendly_target_resolves_proxy_and_network_refs()
    {
        Assert.Equal("target nwk-ep-ubuntu-edne (proxied)",
            TestRunsEndpoints.FriendlyTarget("proxy", """{"kind":"proxy","proxy_endpoint_id":"x"}""", "nwk-ep-ubuntu-edne"));
        Assert.Equal("example.com:8443",
            TestRunsEndpoints.FriendlyTarget("network", """{"kind":"network","host":"example.com","port":8443}""", null));
        Assert.Equal("https://example.com",
            TestRunsEndpoints.FriendlyTarget("network", "https://example.com", null)); // non-JSON kept
    }

    [Fact]
    public void Envelope_load_parses_and_tolerates_garbage()
    {
        var (peak, cores) = TestRunsEndpoints.ParseEnvelopeLoad(
            """{"client_load_before":{"load_avg_1m":0.01},"client_load_after":{"load_avg_1m":0.21},"client_info":{"cpu_cores":2}}""");
        Assert.Equal(0.21, peak!.Value, 5);
        Assert.Equal(2, cores);
        Assert.Equal((null, null), TestRunsEndpoints.ParseEnvelopeLoad("not json"));
        Assert.Equal((null, null), TestRunsEndpoints.ParseEnvelopeLoad(null));
    }
}
