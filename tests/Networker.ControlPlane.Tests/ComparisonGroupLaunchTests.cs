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
}
