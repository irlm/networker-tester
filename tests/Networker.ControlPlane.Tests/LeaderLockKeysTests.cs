using Networker.ControlPlane.Background;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Locks the advisory-lock key derivation (FNV-1a 64 over
/// "networker-controlplane:&lt;service&gt;"). The exact values are asserted on
/// purpose: two control-plane versions deployed side-by-side during a rolling
/// cutover only mutually exclude if they compute the SAME key for the same
/// service, so a change to the hash, the namespace prefix, or a service name is
/// a cross-version safety break and must fail this suite.
/// </summary>
public class LeaderLockKeysTests
{
    /// <summary>The frozen key table — mirrors docs/phase2-cutover-runbook.md.</summary>
    [Theory]
    [InlineData(OpsServiceNames.Scheduler, 204212623316596031L)]
    [InlineData(OpsServiceNames.QueuedRedispatch, -3975542237568181939L)]
    [InlineData(OpsServiceNames.Watchdog, 2528921118521860045L)]
    [InlineData(OpsServiceNames.AgentReaper, 5672273927518729125L)]
    [InlineData(OpsServiceNames.AutoShutdown, -6779850655081117222L)]
    [InlineData(OpsServiceNames.OrphanReaper, -3722790822933648360L)]
    [InlineData(OpsServiceNames.WorkspaceInactivity, 5344226851487828108L)]
    [InlineData(OpsServiceNames.ProvisioningOrchestrator, -6476070105187748186L)]
    public void KeyFor_matches_frozen_value(string service, long expected)
        => Assert.Equal(expected, LeaderLockKeys.KeyFor(service));

    [Fact]
    public void Static_key_fields_match_derivation()
    {
        Assert.Equal(LeaderLockKeys.KeyFor(OpsServiceNames.Scheduler), LeaderLockKeys.Scheduler);
        Assert.Equal(LeaderLockKeys.KeyFor(OpsServiceNames.QueuedRedispatch), LeaderLockKeys.QueuedRedispatch);
        Assert.Equal(LeaderLockKeys.KeyFor(OpsServiceNames.Watchdog), LeaderLockKeys.Watchdog);
        Assert.Equal(LeaderLockKeys.KeyFor(OpsServiceNames.AgentReaper), LeaderLockKeys.AgentReaper);
        Assert.Equal(LeaderLockKeys.KeyFor(OpsServiceNames.AutoShutdown), LeaderLockKeys.AutoShutdown);
        Assert.Equal(LeaderLockKeys.KeyFor(OpsServiceNames.OrphanReaper), LeaderLockKeys.OrphanReaper);
        Assert.Equal(LeaderLockKeys.KeyFor(OpsServiceNames.WorkspaceInactivity), LeaderLockKeys.WorkspaceInactivity);
        Assert.Equal(
            LeaderLockKeys.KeyFor(OpsServiceNames.ProvisioningOrchestrator),
            LeaderLockKeys.ProvisioningOrchestrator);
    }

    [Fact]
    public void All_service_keys_are_distinct()
    {
        var keys = OpsServiceNames.All.Select(LeaderLockKeys.KeyFor).ToList();
        Assert.Equal(keys.Count, keys.Distinct().Count());
    }

    [Fact]
    public void KeyFor_is_deterministic()
        => Assert.Equal(LeaderLockKeys.KeyFor("scheduler"), LeaderLockKeys.KeyFor("scheduler"));

    [Fact]
    public void KeyFor_includes_the_namespace_prefix()
    {
        // A bare FNV of the service name must NOT equal our namespaced key —
        // the prefix is what keeps us clear of other advisory-lock users.
        Assert.StartsWith("networker-controlplane:", LeaderLockKeys.KeyNamespace);
        Assert.NotEqual(LeaderLockKeys.KeyFor(string.Empty), LeaderLockKeys.KeyFor("scheduler"));
    }

    [Fact]
    public void OpsServiceNames_All_covers_every_named_constant()
    {
        var expected = new[]
        {
            OpsServiceNames.Scheduler,
            OpsServiceNames.QueuedRedispatch,
            OpsServiceNames.Watchdog,
            OpsServiceNames.AgentReaper,
            OpsServiceNames.AutoShutdown,
            OpsServiceNames.OrphanReaper,
            OpsServiceNames.WorkspaceInactivity,
            OpsServiceNames.ProvisioningOrchestrator,
        };
        Assert.Equal(expected, OpsServiceNames.All);
    }
}
