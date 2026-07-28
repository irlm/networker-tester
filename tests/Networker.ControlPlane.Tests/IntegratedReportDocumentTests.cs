using System.Text;
using Networker.ControlPlane.Endpoints;
using Networker.ControlPlane.Reports;
using Networker.ControlPlane.Reports.Documents;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Covers the project-level Integrated Test Report builder: section
/// composition over full/partial/empty inputs and the summary math. When
/// <c>REPORT_SMOKE_DIR</c> is set, the HTML/PDF renders are also written there
/// for eyeball inspection (no-op in CI).
/// </summary>
public class IntegratedReportDocumentTests
{
    private static IntegratedReportInput FullInput() => new(
        ProjectId: "acme",
        ProjectName: "Acme Prod",
        RunStatuses: new[]
        {
            new RunStatusCount("completed", 18),
            new RunStatusCount("failed", 2),
            new RunStatusCount("queued", 1),
        },
        FirstRunAt: new DateTime(2026, 7, 1, 8, 0, 0, DateTimeKind.Utc),
        LastRunAt: new DateTime(2026, 7, 28, 14, 0, 0, DateTimeKind.Utc),
        Configs: new[]
        {
            new ConfigResult(Guid.NewGuid(), "prod-http3-sweep", "http3", 12,
                new DateTime(2026, 7, 28, 13, 0, 0, DateTimeKind.Utc), 1200, 1194, 71.2, 150.4),
            new ConfigResult(Guid.NewGuid(), "checkout-api-probe", "sdkprobe", 8,
                new DateTime(2026, 7, 27, 9, 0, 0, DateTimeKind.Utc), 800, 780, 98.0, 201.0),
        },
        Protocols: new[]
        {
            new ProtocolResult("http3", 1200, 1194, 28, 55, 71.2, 110, 150.4, 190),
            new ProtocolResult("sdkprobe", 800, 780, 40, 72, 98.0, 150, 201.0, 260),
        },
        AppNetwork: new AppNetworkReport(
            GeneratedAt: DateTime.UtcNow,
            Formulas: new AppNetworkFormulas("s", "n", "sp", "a"),
            Mode: "sdkprobe",
            AttemptCount: 800,
            SplitAnomalyCount: 0,
            OverallVerdict: AppNetworkLogic.VerdictNetworkBound,
            OverallMainIssue: "network dominates the request time",
            OverallMedianServerMs: 12,
            OverallMedianNetworkMs: 86,
            OverallMedianWallMs: 98,
            OverallServerRatio: 0.12,
            Groups: new[]
            {
                new AppNetworkGroup(Guid.NewGuid(), "checkout-api-probe", 8, 800, 0,
                    12, 20, 86, 140, 98, 0.12, AppNetworkLogic.VerdictNetworkBound, "network dominates"),
            }),
        PerfPerCost: new PerfPerCostReport(
            GeneratedAt: DateTime.UtcNow,
            CostTable: new CostTableInfo("2026-07", "Estimates only.", "shared/cloud-costs.json"),
            Formulas: new FormulasInfo("lci", "mpd"),
            CompletedRuns: 18,
            ProvidersWithData: 1,
            Groups: new[]
            {
                new PerfPerCostGroup("azure", "B2s", "eastus", 0.0416m, "eastus", null, "2026-07", null,
                    new[]
                    {
                        new PerfPerCostFamily("http", 12, 1200, "latency_ms", 71.2, 150.4, "latency_cost_index", 159.8),
                    }),
            },
            MissingCostSkus: Array.Empty<MissingCostSku>()),
        RecentRuns: new[]
        {
            new RecentRunRow(Guid.NewGuid(), "prod-http3-sweep", "completed",
                new DateTime(2026, 7, 28, 13, 0, 0, DateTimeKind.Utc),
                new DateTime(2026, 7, 28, 13, 0, 42, DateTimeKind.Utc), 100, 0),
            new RecentRunRow(Guid.NewGuid(), "checkout-api-probe", "failed",
                new DateTime(2026, 7, 27, 9, 0, 0, DateTimeKind.Utc),
                new DateTime(2026, 7, 27, 9, 1, 0, DateTimeKind.Utc), 88, 12),
        });

    [Fact]
    public void Full_input_produces_summary_plus_all_detail_sections_in_order()
    {
        var doc = IntegratedReportDocument.Build(FullInput());

        Assert.Equal("Integrated Test Report", doc.Title);
        Assert.Equal(new[]
        {
            "Executive summary",
            "Results by test",
            "Latency by protocol",
            "Application vs network",
            "Performance per cost",
            "Recent runs",
        }, doc.Sections.Select(s => s.Heading).ToArray());

        WriteSmokeArtifacts(doc);
    }

    [Fact]
    public void Summary_math_and_verdict_flow_into_the_render()
    {
        var md = Render(FullInput());

        // 2000 attempts, 1974 ok → 98.7%; 2 failed runs drive a Bad verdict.
        Assert.Contains("98.7%", md);
        Assert.Contains("2 of 21 runs failed", md);
        // The app-vs-network headline reaches the executive summary.
        Assert.Contains("Network-bound", md);
        // Detail sections carry the data.
        Assert.Contains("prod-http3-sweep", md);
        Assert.Contains("azure · B2s · eastus", md);
    }

    [Fact]
    public void Analysis_sections_are_omitted_without_data()
    {
        var input = FullInput() with
        {
            AppNetwork = null,
            PerfPerCost = null,
        };

        var doc = IntegratedReportDocument.Build(input);

        Assert.DoesNotContain(doc.Sections, s => s.Heading == "Application vs network");
        Assert.DoesNotContain(doc.Sections, s => s.Heading == "Performance per cost");
    }

    [Fact]
    public void Appnetwork_with_zero_attempts_is_treated_as_no_data()
    {
        var input = FullInput();
        input = input with
        {
            AppNetwork = input.AppNetwork! with { AttemptCount = 0 },
        };

        var doc = IntegratedReportDocument.Build(input);

        Assert.DoesNotContain(doc.Sections, s => s.Heading == "Application vs network");
    }

    [Fact]
    public void Empty_project_renders_a_valid_document_with_a_neutral_verdict()
    {
        var input = new IntegratedReportInput(
            "p", "Empty Project", Array.Empty<RunStatusCount>(), null, null,
            Array.Empty<ConfigResult>(), Array.Empty<ProtocolResult>(),
            AppNetwork: null, PerfPerCost: null, Array.Empty<RecentRunRow>());

        var doc = IntegratedReportDocument.Build(input);
        var md = Render(input);

        Assert.Single(doc.Sections); // executive summary only
        Assert.Contains("No test runs yet", md);
    }

    [Fact]
    public void All_exporters_render_the_integrated_document()
    {
        var doc = IntegratedReportDocument.Build(FullInput());
        foreach (var exporter in new IReportExporter[]
        {
            new MarkdownReportExporter(), new HtmlReportExporter(),
            new DocxReportExporter(), new PdfReportExporter(),
        })
        {
            var bytes = exporter.Render(doc);
            Assert.NotEmpty(bytes);
        }
    }

    private static string Render(IntegratedReportInput input) =>
        Encoding.UTF8.GetString(
            new MarkdownReportExporter().Render(IntegratedReportDocument.Build(input)));

    private static void WriteSmokeArtifacts(ReportDocument doc)
    {
        var dir = Environment.GetEnvironmentVariable("REPORT_SMOKE_DIR");
        if (string.IsNullOrWhiteSpace(dir))
        {
            return;
        }
        Directory.CreateDirectory(dir);
        File.WriteAllBytes(Path.Combine(dir, "integrated.html"),
            new HtmlReportExporter().Render(doc));
        File.WriteAllBytes(Path.Combine(dir, "integrated.pdf"),
            new PdfReportExporter().Render(doc));
        File.WriteAllBytes(Path.Combine(dir, "integrated.md"),
            new MarkdownReportExporter().Render(doc));
    }
}
