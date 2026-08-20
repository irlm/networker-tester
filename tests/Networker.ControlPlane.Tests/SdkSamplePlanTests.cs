using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The reuse / update / redeploy decision, pinned.
///
/// <para>The owner's brief was specific: reuse must be the default when
/// something usable exists (it is the cheap path), an outdated sample must be
/// offered as an in-place UPDATE with both versions named, and a stale or
/// unhealthy endpoint must NEVER be presented as usable. Each of those is a
/// separate test here so a future refactor cannot quietly soften one.</para>
/// </summary>
public class SdkSamplePlanTests
{
    private static SdkSampleCatalog.Sample Go => SdkSampleCatalog.Find("go")!;

    private static SdkSamplePlan.Deployed Deployed(
        string status = "completed",
        string? host = "10.0.0.4",
        Guid? registered = null,
        int samplesOnHost = 1) =>
        new(Guid.NewGuid(), "sdk-go-ab12", status, host, "azure", "eastus", "Standard_B2s",
            samplesOnHost > 1, samplesOnHost, registered);

    [Fact]
    public void Nothing_deployed_is_create()
    {
        var v = SdkSamplePlan.Decide(Go, null, null);
        Assert.Equal(SdkSamplePlan.State.None, v.StateOf);
        Assert.Equal(SdkSamplePlan.Action.Create, v.Recommended);
        Assert.False(v.Reusable);
        Assert.Null(v.DeployedVersion);
    }

    [Fact]
    public void A_matching_version_is_current_and_reused_not_redeployed()
    {
        var v = SdkSamplePlan.Decide(Go, Deployed(), new SdkSamplePlan.Probe(true, Go.SdkVersion, "go"));
        Assert.Equal(SdkSamplePlan.State.Current, v.StateOf);
        Assert.Equal(SdkSamplePlan.Action.Reuse, v.Recommended);
        Assert.True(v.Reusable);
        Assert.Equal(Go.SdkVersion, v.DeployedVersion);
        Assert.Contains("no new server", v.Reason, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void An_older_deployed_version_is_outdated_and_names_both_versions()
    {
        var v = SdkSamplePlan.Decide(Go, Deployed(), new SdkSamplePlan.Probe(true, "0.0.1", "go"));
        Assert.Equal(SdkSamplePlan.State.Outdated, v.StateOf);
        Assert.Equal(SdkSamplePlan.Action.Update, v.Recommended);
        // Reusable: an outdated sample still serves, so a create must not
        // provision a second server for it behind the user's back.
        Assert.True(v.Reusable);
        Assert.Equal("0.0.1", v.DeployedVersion);
        Assert.Contains("0.0.1", v.Reason, StringComparison.Ordinal);
        Assert.Contains(Go.SdkVersion, v.Reason, StringComparison.Ordinal);
    }

    [Fact]
    public void A_newer_deployed_version_is_reused_never_downgraded()
    {
        // A host deployed from a newer tree than this control plane's build.
        var v = SdkSamplePlan.Decide(Go, Deployed(), new SdkSamplePlan.Probe(true, "99.0.0", "go"));
        Assert.Equal(SdkSamplePlan.State.Current, v.StateOf);
        Assert.Equal(SdkSamplePlan.Action.Reuse, v.Recommended);
        Assert.Contains("nothing to update", v.Reason, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void An_unreachable_sample_is_never_usable()
    {
        var v = SdkSamplePlan.Decide(Go, Deployed(), new SdkSamplePlan.Probe(false, null, null));
        Assert.Equal(SdkSamplePlan.State.Unhealthy, v.StateOf);
        Assert.Equal(SdkSamplePlan.Action.Redeploy, v.Recommended);
        Assert.False(v.Reusable);
        Assert.Null(v.DeployedVersion);
    }

    [Fact]
    public void A_never_probed_completed_deployment_is_unhealthy_not_assumed_fine()
    {
        var v = SdkSamplePlan.Decide(Go, Deployed(), probe: null);
        Assert.Equal(SdkSamplePlan.State.Unhealthy, v.StateOf);
        Assert.False(v.Reusable);
    }

    [Fact]
    public void A_completed_deployment_with_no_host_is_unhealthy()
    {
        var v = SdkSamplePlan.Decide(Go, Deployed(host: null), new SdkSamplePlan.Probe(true, Go.SdkVersion, "go"));
        Assert.Equal(SdkSamplePlan.State.Unhealthy, v.StateOf);
        Assert.Equal(SdkSamplePlan.Action.Redeploy, v.Recommended);
    }

    [Fact]
    public void A_failed_deployment_is_redeployed_and_says_nothing_is_serving()
    {
        foreach (var status in new[] { "failed", "cancelled", "torn_down" })
        {
            var v = SdkSamplePlan.Decide(Go, Deployed(status), null);
            Assert.Equal(SdkSamplePlan.State.Failed, v.StateOf);
            Assert.Equal(SdkSamplePlan.Action.Redeploy, v.Recommended);
            Assert.False(v.Reusable);
            Assert.Contains("nothing is serving", v.Reason, StringComparison.OrdinalIgnoreCase);
        }
    }

    [Fact]
    public void An_in_flight_deployment_is_waited_on()
    {
        foreach (var status in new[] { "pending", "running", "provisioning" })
        {
            var v = SdkSamplePlan.Decide(Go, Deployed(status), null);
            Assert.Equal(SdkSamplePlan.State.Deploying, v.StateOf);
            Assert.Equal(SdkSamplePlan.Action.Wait, v.Recommended);
            Assert.False(v.Reusable);
        }
    }

    [Fact]
    public void A_live_sample_with_an_unreadable_version_is_reusable_but_never_called_current()
    {
        var v = SdkSamplePlan.Decide(Go, Deployed(), new SdkSamplePlan.Probe(true, null, null));
        Assert.Equal(SdkSamplePlan.State.UnknownVersion, v.StateOf);
        Assert.Equal(SdkSamplePlan.Action.Reuse, v.Recommended);
        Assert.True(v.Reusable);
        Assert.Null(v.DeployedVersion);
        Assert.NotEqual(SdkSamplePlan.State.Current, v.StateOf);
    }

    // ── Split / shape arithmetic ─────────────────────────────────────────────

    private static Dictionary<string, SdkSamplePlan.Verdict> Verdicts(params (string Lang, bool Reusable)[] rows)
    {
        var map = new Dictionary<string, SdkSamplePlan.Verdict>(StringComparer.Ordinal);
        foreach (var (lang, reusable) in rows)
        {
            map[lang] = new SdkSamplePlan.Verdict(
                lang, "1.0.0", reusable ? "1.0.0" : null,
                reusable ? SdkSamplePlan.State.Current : SdkSamplePlan.State.None,
                reusable ? SdkSamplePlan.Action.Reuse : SdkSamplePlan.Action.Create,
                "reason", reusable);
        }
        return map;
    }

    [Fact]
    public void Split_reuses_what_it_can_and_provisions_the_rest()
    {
        var (reuse, provision) = SdkSamplePlan.Split(
            ["go", "rust", "python"],
            Verdicts(("go", true), ("rust", false), ("python", true)),
            reuseExisting: true);
        Assert.Equal(["go", "python"], reuse);
        Assert.Equal(["rust"], provision);
    }

    [Fact]
    public void Split_with_reuse_off_provisions_everything_explicitly()
    {
        var (reuse, provision) = SdkSamplePlan.Split(
            ["go", "rust"], Verdicts(("go", true), ("rust", false)), reuseExisting: false);
        Assert.Empty(reuse);
        Assert.Equal(["go", "rust"], provision);
    }

    [Fact]
    public void Server_count_is_one_for_consolidated_and_n_for_separated()
    {
        Assert.Equal(1, SdkSamplePlan.ServerCount(SdkSamplePlan.Shape.Consolidated, 5));
        Assert.Equal(5, SdkSamplePlan.ServerCount(SdkSamplePlan.Shape.Separated, 5));
        // Nothing to provision costs nothing, in either shape.
        Assert.Equal(0, SdkSamplePlan.ServerCount(SdkSamplePlan.Shape.Consolidated, 0));
        Assert.Equal(0, SdkSamplePlan.ServerCount(SdkSamplePlan.Shape.Separated, 0));
    }

    [Fact]
    public void Shape_parsing_rejects_anything_else()
    {
        Assert.Equal(SdkSamplePlan.Shape.Consolidated, SdkSamplePlan.ParseShape("Consolidated"));
        Assert.Equal(SdkSamplePlan.Shape.Separated, SdkSamplePlan.ParseShape(" separated "));
        Assert.Null(SdkSamplePlan.ParseShape("cheapest"));
        Assert.Null(SdkSamplePlan.ParseShape(null));
    }

    [Fact]
    public void Wire_names_are_stable_snake_case()
    {
        Assert.Equal("unknown_version", SdkSamplePlan.Wire(SdkSamplePlan.State.UnknownVersion));
        Assert.Equal("outdated", SdkSamplePlan.Wire(SdkSamplePlan.State.Outdated));
        Assert.Equal("redeploy", SdkSamplePlan.Wire(SdkSamplePlan.Action.Redeploy));
        Assert.Equal("wait", SdkSamplePlan.Wire(SdkSamplePlan.Action.Wait));
    }
}
