using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Unit tests for <see cref="ComparisonGroupsEndpoints.ParseCells"/> — the cell
/// JSON → launch-spec parse that drives the comparison-group launch (previously
/// an unimplemented stub that created zero runs; the matrix Application/
/// Full-Stack benchmarks silently redirected to an empty results page). These
/// pin the field mapping; the full create-config-and-dispatch-per-cell flow is
/// verified live against prod with a small 2-cell matrix.
/// </summary>
public class ComparisonGroupLaunchTests
{
    private const string RealCells = """
        [
          { "label": "rust @ Azure/eastus @ linux @ nginx",
            "endpoint": { "kind": "pending", "language": "rust", "proxy_stack": "nginx",
                          "region": "eastus", "vm_size": "Standard_B2s",
                          "cloud_account_id": "57ecde0d-5e00-49de-8142-2b14bba24347" },
            "runner_id": "2a1cafc1-9f0b-4c9a-8fcc-aa99ea06137a" },
          { "label": "go @ Azure/eastus @ linux @ caddy",
            "endpoint": { "kind": "pending", "language": "go", "proxy_stack": "caddy" } }
        ]
        """;

    [Fact]
    public void Parses_real_wizard_cells()
    {
        var cells = ComparisonGroupsEndpoints.ParseCells(RealCells);

        Assert.Equal(2, cells.Count);

        Assert.Equal("rust @ Azure/eastus @ linux @ nginx", cells[0].Label);
        Assert.Equal("pending", cells[0].EndpointKind);
        Assert.Equal(Guid.Parse("2a1cafc1-9f0b-4c9a-8fcc-aa99ea06137a"), cells[0].RunnerId);
        Assert.Contains("\"proxy_stack\": \"nginx\"", cells[0].EndpointRaw); // endpoint preserved verbatim

        // A cell may omit runner_id (auto-pick) — must parse, not throw.
        Assert.Null(cells[1].RunnerId);
        Assert.Equal("go @ Azure/eastus @ linux @ caddy", cells[1].Label);
    }

    [Fact]
    public void Skips_cells_without_an_endpoint_object()
    {
        var cells = ComparisonGroupsEndpoints.ParseCells("""
            [ { "label": "no endpoint" },
              { "label": "endpoint not an object", "endpoint": "oops" },
              { "label": "ok", "endpoint": { "kind": "network", "host": "example.com", "port": 443 } } ]
            """);

        Assert.Single(cells);
        Assert.Equal("ok", cells[0].Label);
        Assert.Equal("network", cells[0].EndpointKind);
    }

    [Theory]
    [InlineData("[]")]                                  // empty matrix
    [InlineData("""{"not":"an array"}""")]              // object, not array
    public void Empty_or_non_array_yields_no_cells(string json)
        => Assert.Empty(ComparisonGroupsEndpoints.ParseCells(json));

    [Fact]
    public void Defaults_missing_kind_to_pending_and_missing_label_to_cell()
    {
        var cells = ComparisonGroupsEndpoints.ParseCells("""
            [ { "endpoint": { "language": "rust" } } ]
            """);

        Assert.Single(cells);
        Assert.Equal("cell", cells[0].Label);
        Assert.Equal("pending", cells[0].EndpointKind);
    }

    // ── Per-cell HTTP/3 trim (shared/http-stacks.json h3) ────────────────────

    private const string MatrixWorkload =
        """{"modes":["http1","http2","http3","pageload3","download","upload"],"runs":10,"concurrency":1,"timeout_ms":5000,"payload_sizes":[1048576],"capture_mode":"metrics-only"}""";

    private static ComparisonGroupsEndpoints.CellSpec Cell(string stack, string kind = "pending") =>
        new($"linux · {stack}", $$"""{"kind":"{{kind}}","cloud_account_id":"57ecde0d-5e00-49de-8142-2b14bba24347","region":"eastus","vm_size":"Standard_B2s","os":"linux","proxy_stack":"{{stack}}","topology":"loopback"}""", kind, null);

    [Fact]
    public void Apache_cell_drops_h3_modes_and_keeps_the_rest()
    {
        var (workload, dropped) = ComparisonGroupsEndpoints.TrimH3ModesForCell(MatrixWorkload, Cell("apache"));

        Assert.Equal(["http3", "pageload3"], dropped);
        Assert.NotNull(workload);
        using var doc = System.Text.Json.JsonDocument.Parse(workload!);
        var modes = doc.RootElement.GetProperty("modes").EnumerateArray().Select(m => m.GetString()).ToArray();
        Assert.Equal(["http1", "http2", "download", "upload"], modes);
        // Everything else in the workload survives untouched.
        Assert.Equal(10, doc.RootElement.GetProperty("runs").GetInt32());
        Assert.Equal(1048576, doc.RootElement.GetProperty("payload_sizes")[0].GetInt32());
    }

    [Theory]
    [InlineData("nginx")]
    [InlineData("caddy")]
    [InlineData("envoy")] // unknown → fail open, run everything
    public void Quic_capable_or_unknown_stacks_keep_the_base_workload(string stack)
    {
        var (workload, dropped) = ComparisonGroupsEndpoints.TrimH3ModesForCell(MatrixWorkload, Cell(stack));
        Assert.Null(workload);
        Assert.Empty(dropped);
    }

    [Fact]
    public void Cell_whose_every_mode_needs_h3_reports_all_dropped_and_no_workload()
    {
        var (workload, dropped) = ComparisonGroupsEndpoints.TrimH3ModesForCell(
            """{"modes":["http3","browser3"],"runs":3}""", Cell("haproxy"));
        Assert.Null(workload);
        Assert.Equal(["http3", "browser3"], dropped);
    }

    [Fact]
    public void Proxy_kind_cell_with_a_stack_override_is_trimmed_too()
    {
        var cell = new ComparisonGroupsEndpoints.CellSpec(
            "existing traefik", """{"kind":"proxy","proxy_endpoint_id":"2a1cafc1-9f0b-4c9a-8fcc-aa99ea06137a","proxy_stack":"traefik"}""", "proxy", null);
        var (_, dropped) = ComparisonGroupsEndpoints.TrimH3ModesForCell(MatrixWorkload, cell);
        Assert.Equal(["http3", "pageload3"], dropped);
        Assert.Equal("traefik", ComparisonGroupsEndpoints.CellProxyStack(cell));
    }

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("not json")]
    [InlineData("""{"runs":3}""")]
    public void Malformed_or_mode_less_workloads_are_left_alone(string? workload)
    {
        var (rewritten, dropped) = ComparisonGroupsEndpoints.TrimH3ModesForCell(workload, Cell("apache"));
        Assert.Null(rewritten);
        Assert.Empty(dropped);
    }
}
