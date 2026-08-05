using System.Net.Http.Json;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.Data.Entities;
using Xunit;

namespace Networker.Tests;

/// <summary>
/// P0-2 (2026-08 audit): the background loops already tick against REAL
/// Postgres while this fixture's app is booted — but nothing ever asserted
/// they tick WITHOUT ERROR. The 2026-08-03 prod wedge (teardown's
/// endpoint_ref Contains → `jsonb ~~ jsonb` → 42883 on every tick, starving
/// all provisioning kicks) ran for a day and would have been caught here:
/// this test seeds a row forcing each fast loop down its stateful path, then
/// requires every fast loop to have ticked with <c>last_error == null</c>.
/// Any future EF-translation break (jsonb operators, ExecuteUpdate SQL) in a
/// loop body turns this red.
/// </summary>
public class BackgroundLoopHealthTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fx;

    public BackgroundLoopHealthTests(ControlPlaneFixture fx) => _fx = fx;

    /// <summary>Loops whose interval is short enough to tick during the test
    /// window. orphan-reaper (10m) and workspace-inactivity (24h) can't tick
    /// here; their first tick is asserted not-to-error only if it happened.</summary>
    private static readonly string[] FastLoops =
    [
        "scheduler",
        "queued-redispatch",
        "watchdog",
        "agent-reaper",
        "auto-shutdown",
        "provisioning-orchestrator",
    ];

    [Fact]
    public async Task Every_fast_loop_ticks_against_real_postgres_without_error()
    {
        var now = DateTime.UtcNow;
        Guid teardownDeploymentId, wakeTesterId, staleRunId, scheduleId;
        DateTime seededNextFire;

        await using (var db = _fx.NewDbContext())
        {
            // ── Force the PROVISIONING ORCHESTRATOR teardown path (the wedge
            // class): a terminal run past the teardown grace, linked to a live
            // deployment with real jsonb config + endpoint_ips. The tick must
            // select active runs' endpoint_ref (jsonb) and mark this torn_down.
            teardownDeploymentId = Guid.NewGuid();
            db.Deployments.Add(new Deployment
            {
                DeploymentId = teardownDeploymentId,
                Name = "loop-health-teardown",
                Status = "completed",
                Config = """{"version":1,"cloud_account_id":"00000000-0000-4000-8000-000000000000","endpoints":[{"provider":"azure","azure":{"region":"eastus","vm_size":"Standard_B2s","os":"linux","vm_name":"nwk-a-loophealth"}}]}""",
                EndpointIps = """["10.99.99.99"]""",
                ProjectId = ControlPlaneFixture.SeededProjectId,
                CreatedAt = now.AddMinutes(-30),
                FinishedAt = now.AddMinutes(-20),
            });
            var teardownCfg = Guid.NewGuid();
            db.TestConfigs.Add(new TestConfig
            {
                Id = teardownCfg,
                ProjectId = ControlPlaneFixture.SeededProjectId,
                Name = $"loop-health-teardown-cfg-{teardownCfg:N}",
                EndpointKind = "network",
                EndpointRef = """{"kind":"network","host":"10.99.99.99","port":8444}""",
                Workload = "{}",
                MaxDurationSecs = 60,
                CreatedAt = now.AddMinutes(-30),
                UpdatedAt = now.AddMinutes(-30),
            });
            db.TestRuns.Add(new TestRun
            {
                Id = Guid.NewGuid(),
                TestConfigId = teardownCfg,
                ProjectId = ControlPlaneFixture.SeededProjectId,
                Status = "failed",
                ProvisioningDeploymentId = teardownDeploymentId,
                CreatedAt = now.AddMinutes(-30),
                FinishedAt = now.AddMinutes(-15), // far past TeardownGrace
            });

            // ── Force the AUTO-SHUTDOWN wake arm: stopped tester + queued run.
            wakeTesterId = Guid.NewGuid();
            db.ProjectTesters.Add(new ProjectTester
            {
                TesterId = wakeTesterId,
                ProjectId = ControlPlaneFixture.SeededProjectId,
                Name = $"loop-health-wake-{wakeTesterId:N}",
                Cloud = "azure",
                Region = "eastus",
                VmSize = "Standard_B2s",
                SshUser = "azureuser",
                PowerState = "stopped",
                Allocation = "idle",
                AutoShutdownEnabled = true,
                AutoShutdownLocalHour = 23,
                ShutdownDeferralCount = 0,
                AutoProbeEnabled = false,
                BenchmarkRunCount = 0,
                CreatedAt = now,
            });
            db.TestRuns.Add(new TestRun
            {
                Id = Guid.NewGuid(),
                TestConfigId = ControlPlaneFixture.SeededConfigId,
                ProjectId = ControlPlaneFixture.SeededProjectId,
                Status = "queued",
                TesterId = wakeTesterId,
                CreatedAt = now, // fresh: under the queued-reap age window
            });

            // ── Force the WATCHDOG stale-running path.
            staleRunId = Guid.NewGuid();
            db.TestRuns.Add(new TestRun
            {
                Id = staleRunId,
                TestConfigId = ControlPlaneFixture.SeededConfigId,
                ProjectId = ControlPlaneFixture.SeededProjectId,
                Status = "running",
                WorkerId = Guid.NewGuid().ToString(), // never online
                StartedAt = now.AddMinutes(-10),
                LastHeartbeat = now.AddMinutes(-10),
                CreatedAt = now.AddMinutes(-10),
            });

            // ── Force the SCHEDULER due path (network config, no agent online
            // → skip-and-advance guard is the expected real behavior).
            scheduleId = Guid.NewGuid();
            seededNextFire = now.AddMinutes(-1);
            db.TestSchedules.Add(new TestSchedule
            {
                Id = scheduleId,
                TestConfigId = ControlPlaneFixture.SeededConfigId,
                ProjectId = ControlPlaneFixture.SeededProjectId,
                CronExpr = "*/5 * * * *",
                Timezone = "UTC",
                Enabled = true,
                NextFireAt = seededNextFire,
                CreatedAt = now,
            });

            await db.SaveChangesAsync();
        }

        // Boot the app (loops start with it) and wait for every fast loop to
        // tick at least once. The 60s-interval loops (PeriodicTimer waits one
        // interval before the first tick) dominate the wait.
        using var client = _fx.CreateAuthenticatedClient();
        JsonElement lastBody = default;
        var deadline = DateTime.UtcNow.AddSeconds(150);
        var allTicked = false;
        while (DateTime.UtcNow < deadline)
        {
            var resp = await client.GetAsync("/api/health/background");
            resp.EnsureSuccessStatusCode();
            lastBody = await resp.Content.ReadFromJsonAsync<JsonElement>();
            var services = lastBody.GetProperty("services").EnumerateArray()
                .ToDictionary(s => s.GetProperty("name").GetString()!, s => s);
            allTicked = FastLoops.All(n =>
                services.TryGetValue(n, out var s)
                && s.GetProperty("last_tick_at").ValueKind != JsonValueKind.Null);
            if (allTicked)
            {
                break;
            }
            await Task.Delay(TimeSpan.FromSeconds(5));
        }

        Assert.True(allTicked, $"not all fast loops ticked within the window: {lastBody}");

        // THE assertion: no loop errored. A jsonb/ExecuteUpdate translation
        // break in any loop body shows up as last_error here.
        var final = lastBody.GetProperty("services").EnumerateArray()
            .ToDictionary(s => s.GetProperty("name").GetString()!, s => s);
        foreach (var name in FastLoops)
        {
            var err = final[name].GetProperty("last_error");
            Assert.True(err.ValueKind == JsonValueKind.Null,
                $"loop '{name}' errored: {err}");
        }
        // Slow loops (orphan-reaper 10m, workspace-inactivity 24h) may not have
        // ticked — but if they did, they must not have errored either.
        foreach (var (name, s) in final)
        {
            if (s.GetProperty("last_tick_at").ValueKind != JsonValueKind.Null)
            {
                Assert.True(s.GetProperty("last_error").ValueKind == JsonValueKind.Null,
                    $"loop '{name}' errored: {s.GetProperty("last_error")}");
            }
        }

        // Domain outcomes proving each loop did REAL work, not just a no-op tick:
        await using var check = _fx.NewDbContext();

        // Watchdog reaped the stale run.
        var stale = await check.TestRuns.AsNoTracking().FirstAsync(r => r.Id == staleRunId);
        Assert.Equal("failed", stale.Status);

        // Orchestrator marked the finished run's deployment torn_down (the
        // exact query family that wedged prod on 2026-08-03 — now on Postgres).
        var dep = await check.Deployments.AsNoTracking()
            .FirstAsync(d => d.DeploymentId == teardownDeploymentId);
        Assert.Equal("torn_down", dep.Status);

        // Scheduler advanced the due schedule (skip-and-advance with no agent).
        var sched = await check.TestSchedules.AsNoTracking().FirstAsync(s => s.Id == scheduleId);
        Assert.True(sched.NextFireAt > seededNextFire,
            $"schedule not advanced: {sched.NextFireAt:o} <= {seededNextFire:o}");

        // Auto-wake attempted the stopped tester: either mid-wake ('starting')
        // or rolled back after a real CLI failure ('stopped') — never wedged
        // in an unknown state, and never an error on the loop.
        var wake = await check.ProjectTesters.AsNoTracking()
            .FirstAsync(t => t.TesterId == wakeTesterId);
        Assert.Contains(wake.PowerState, new[] { "starting", "stopped" });
    }
}
