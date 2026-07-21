using Networker.ControlPlane.Reports;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Verdict + split math for the Application Network Performance report. This is
/// the product surface — a wrong verdict tells a customer to debug the wrong
/// half of their stack — so every branch and its boundary are pinned with
/// hand-picked numbers, DB-free.
/// </summary>
public sealed class AppNetworkLogicTests
{
    // ── network_ms = max(0, wall - server) ──────────────────────────────────

    [Theory]
    [InlineData(200.0, 150.0, 50.0)]  // normal split
    [InlineData(100.0, 100.0, 0.0)]   // all server
    [InlineData(100.0, 120.0, 0.0)]   // anomaly floors at 0, never negative
    public void Network_ms_is_wall_minus_server_floored_at_zero(double wall, double server, double expected)
    {
        Assert.Equal(expected, AppNetworkLogic.NetworkMs(wall, server));
    }

    [Theory]
    [InlineData(100.0, 120.0, true)]   // server > wall → anomaly
    [InlineData(120.0, 120.0, false)]  // equal → not an anomaly
    [InlineData(200.0, 150.0, false)]  // normal
    public void Split_anomaly_is_server_over_wall(double wall, double server, bool expected)
    {
        Assert.Equal(expected, AppNetworkLogic.IsSplitAnomaly(wall, server));
    }

    // ── verdict ─────────────────────────────────────────────────────────────

    [Theory]
    // server >= 60% wall → server_bound (checked first)
    [InlineData(150.0, 50.0, 200.0, "server_bound")]
    // server exactly at the 60% boundary → server_bound (>= is inclusive)
    [InlineData(120.0, 80.0, 200.0, "server_bound")]
    // network >= 60% wall, server below → network_bound
    [InlineData(40.0, 160.0, 200.0, "network_bound")]
    // network exactly at the 60% boundary, server below → network_bound
    [InlineData(80.0, 120.0, 200.0, "network_bound")]
    // neither dominates (both under 60%) → balanced
    [InlineData(100.0, 100.0, 200.0, "balanced")]
    public void Verdict_classifies_from_the_medians(
        double server, double network, double wall, string expected)
    {
        Assert.Equal(expected, AppNetworkLogic.Verdict(server, network, wall));
    }

    [Fact]
    public void Verdict_prefers_server_when_both_sides_would_qualify()
    {
        // Degenerate case (skew): both >= 60% of wall — server wins the headline.
        Assert.Equal("server_bound", AppNetworkLogic.Verdict(150.0, 150.0, 200.0));
    }

    [Theory]
    [InlineData(null, 40.0, 200.0)]   // no server median
    [InlineData(150.0, null, 200.0)]  // no network median
    [InlineData(150.0, 40.0, null)]   // no wall
    [InlineData(150.0, 40.0, 0.0)]    // zero wall (no samples)
    public void Verdict_is_no_data_when_a_median_is_missing(double? server, double? network, double? wall)
    {
        Assert.Equal("no_data", AppNetworkLogic.Verdict(server, network, wall));
    }

    // ── server_ratio ────────────────────────────────────────────────────────

    [Fact]
    public void Server_ratio_is_server_over_wall_capped_at_one()
    {
        Assert.Equal(0.75, AppNetworkLogic.ServerRatio(150.0, 200.0));
        // Anomalous server > wall clamps to 1.0 rather than exceeding it.
        Assert.Equal(1.0, AppNetworkLogic.ServerRatio(250.0, 200.0));
        Assert.Null(AppNetworkLogic.ServerRatio(150.0, 0.0));
        Assert.Null(AppNetworkLogic.ServerRatio(null, 200.0));
    }

    // ── main_issue ──────────────────────────────────────────────────────────

    [Fact]
    public void Main_issue_names_the_application_for_server_bound()
    {
        var msg = AppNetworkLogic.MainIssue("server_bound", 180.0, 40.0, 220.0);
        Assert.Contains("Server processing dominates", msg);
        Assert.Contains("~180ms", msg);
        Assert.Contains("~220ms", msg);
        Assert.Contains("investigate your application", msg);
    }

    [Fact]
    public void Main_issue_names_connectivity_for_network_bound()
    {
        var msg = AppNetworkLogic.MainIssue("network_bound", 40.0, 160.0, 200.0);
        Assert.Contains("Network transit dominates", msg);
        Assert.Contains("~160ms", msg);
        Assert.Contains("connectivity", msg);
    }

    [Fact]
    public void Main_issue_has_a_balanced_and_a_no_data_line()
    {
        Assert.Contains("Balanced", AppNetworkLogic.MainIssue("balanced", 100.0, 100.0, 200.0));
        Assert.Contains("No sdkprobe samples", AppNetworkLogic.MainIssue("no_data", null, null, null));
    }
}
