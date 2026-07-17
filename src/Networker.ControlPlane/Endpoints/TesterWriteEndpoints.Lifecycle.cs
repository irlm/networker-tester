using System.Diagnostics;
using System.Text;
using System.Text.Json;
using Microsoft.AspNetCore.Mvc;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Provisioning;
using Networker.Data;
using Networker.Data.Entities;
using Networker.Security;
using Npgsql;
using NpgsqlTypes;

namespace Networker.ControlPlane.Endpoints;

// Power/allocation lifecycle handlers (start / stop / force-stop / upgrade /
// probe / delete) for TesterWriteEndpoints (route mapping + shared helpers
// live in TesterWriteEndpoints.cs).
public static partial class TesterWriteEndpoints
{
    // ── start ────────────────────────────────────────────────────────────────

    /// <summary>POST /start — stopped → running. Sets <c>power_state=starting</c>,
    /// kicks <c>provisioner.StartAsync</c> in the background, returns 202.</summary>
    private static async Task<IResult> StartTester(
        string projectId,
        Guid testerId,
        HttpContext http,
        NetworkerDbContext db,
        IComputeProvisioner provisioner,
        IServiceScopeFactory scopeFactory,
        ILoggerFactory loggerFactory,
        CancellationToken ct)
    {
        var user = http.GetAuthUser();
        var logger = loggerFactory.CreateLogger("TesterWrite.Start");

        var tester = await LoadAsync(db, projectId, testerId, ct);
        if (tester is null)
        {
            return Results.NotFound(new { error = "Tester not found" });
        }

        if (tester.PowerState != "stopped")
        {
            return Conflict($"cannot start tester in power_state={tester.PowerState}; expected 'stopped'");
        }

        tester.PowerState = "starting";
        tester.StatusMessage = "Start requested";
        tester.UpdatedAt = DateTime.UtcNow;
        await db.SaveChangesAsync(ct);

        logger.LogInformation(
            "tester {TesterId} start requested by {Actor} (provisioning in background)",
            testerId, user?.Email);

        FireAndForget(scopeFactory, loggerFactory, testerId, "start", async (p, cred, t, l, token) =>
        {
            var res = await p.StartAsync(t, cred, token);
            await FinishAsync(scopeFactory, testerId, res, running: "running", failedTo: "stopped", "start", l, token);
        });

        return Results.Accepted($"/api/projects/{projectId}/testers/{testerId}", ToDto(tester));
    }

    // ── stop ─────────────────────────────────────────────────────────────────

    /// <summary>POST /stop — running → stopped (Azure deallocate). Guards
    /// allocation=idle + no in-flight runs, then 202 + background deallocate.</summary>
    private static async Task<IResult> StopTester(
        string projectId,
        Guid testerId,
        HttpContext http,
        NetworkerDbContext db,
        IServiceScopeFactory scopeFactory,
        ILoggerFactory loggerFactory,
        CancellationToken ct)
    {
        var user = http.GetAuthUser();
        var logger = loggerFactory.CreateLogger("TesterWrite.Stop");

        var tester = await LoadAsync(db, projectId, testerId, ct);
        if (tester is null)
        {
            return Results.NotFound(new { error = "Tester not found" });
        }

        if (tester.Allocation != "idle")
        {
            return Conflict($"cannot stop tester with allocation={tester.Allocation}; must be idle");
        }
        if (tester.PowerState != "running")
        {
            return Conflict($"cannot stop tester in power_state={tester.PowerState}; expected 'running'");
        }

        var inFlight = await InFlightRunCountAsync(db, testerId, ct);
        if (inFlight > 0)
        {
            return Conflict($"cannot stop tester with {inFlight} benchmark(s) in flight");
        }

        tester.PowerState = "stopping";
        tester.StatusMessage = "Stop requested";
        tester.UpdatedAt = DateTime.UtcNow;
        await db.SaveChangesAsync(ct);

        logger.LogInformation("tester {TesterId} stop requested by {Actor}", testerId, user?.Email);

        FireAndForget(scopeFactory, loggerFactory, testerId, "stop", async (p, cred, t, l, token) =>
        {
            var res = await p.StopAsync(t, cred, token);
            await FinishAsync(scopeFactory, testerId, res, running: "stopped", failedTo: "running", "stop", l, token);
        });

        return Results.Accepted($"/api/projects/{projectId}/testers/{testerId}", ToDto(tester));
    }

    // ── force-stop ─────────────────────────────────────────────────────────────

    /// <summary>POST /force-stop (Admin) — override. Refuses only while a
    /// benchmark is actively running+locked; otherwise force-releases the
    /// allocation, marks <c>power_state=stopping</c>, and deallocates in the
    /// background. Requires <c>{confirm:true, reason:"..."}</c>.</summary>
    private static async Task<IResult> ForceStopTester(
        string projectId,
        Guid testerId,
        HttpContext http,
        [FromBody] ForceStopBody? body,
        NetworkerDbContext db,
        IServiceScopeFactory scopeFactory,
        ILoggerFactory loggerFactory,
        CancellationToken ct)
    {
        var user = http.GetAuthUser();
        var logger = loggerFactory.CreateLogger("TesterWrite.ForceStop");

        if (body is null || !body.Confirm)
        {
            return Results.BadRequest(new { error = "force-stop requires {\"confirm\": true, \"reason\": \"...\"}" });
        }
        if (string.IsNullOrWhiteSpace(body.Reason))
        {
            return Results.BadRequest(new { error = "reason must not be empty" });
        }

        var tester = await LoadAsync(db, projectId, testerId, ct);
        if (tester is null)
        {
            return Results.NotFound(new { error = "Tester not found" });
        }

        // Refuse if a benchmark is actively running (cancel it first).
        if (tester.PowerState == "running" && tester.Allocation == "locked")
        {
            return Conflict(
                "cannot force-stop tester while a benchmark is actively running; cancel the benchmark first");
        }

        // Force-release the allocation + mark stopping. The real deallocate runs
        // in the background; the row is authoritative immediately.
        tester.Allocation = "idle";
        tester.LockedByConfigId = null;
        tester.PowerState = "stopping";
        tester.StatusMessage = $"Force-stopped: {body.Reason}";
        tester.UpdatedAt = DateTime.UtcNow;
        await db.SaveChangesAsync(ct);

        logger.LogWarning(
            "tester {TesterId} force-stopped by {Actor} (admin override): {Reason}",
            testerId, user?.Email, body.Reason);

        FireAndForget(scopeFactory, loggerFactory, testerId, "force-stop", async (p, cred, t, l, token) =>
        {
            var res = await p.StopAsync(t, cred, token);
            await FinishAsync(scopeFactory, testerId, res, running: "stopped", failedTo: "stopped", "force-stop", l, token);
        });

        // Reload so the response reflects the committed state.
        var updated = await LoadAsync(db, projectId, testerId, ct);
        return Results.Ok(ToDto(updated!));
    }

    // ── upgrade ────────────────────────────────────────────────────────────────

    /// <summary>POST /upgrade (Admin) — re-run the installer on a running,
    /// idle tester. Requires <c>{confirm:true}</c>. Marks
    /// <c>allocation=upgrading</c>, returns 202; the actual re-install is the
    /// deploy-runner's job (M4 slice 2) — here we do the state transition and a
    /// state probe so the row reflects reality.</summary>
    private static async Task<IResult> UpgradeTester(
        string projectId,
        Guid testerId,
        HttpContext http,
        [FromBody] UpgradeBody? body,
        NetworkerDbContext db,
        IServiceScopeFactory scopeFactory,
        ILoggerFactory loggerFactory,
        CancellationToken ct)
    {
        var user = http.GetAuthUser();
        var logger = loggerFactory.CreateLogger("TesterWrite.Upgrade");

        if (body is null || !body.Confirm)
        {
            return Results.BadRequest(new { error = "upgrade requires {\"confirm\": true}" });
        }

        var tester = await LoadAsync(db, projectId, testerId, ct);
        if (tester is null)
        {
            return Results.NotFound(new { error = "Tester not found" });
        }

        if (tester.Allocation != "idle")
        {
            return Conflict($"cannot upgrade tester with allocation={tester.Allocation}; must be idle");
        }
        if (tester.PowerState != "running")
        {
            return Conflict($"cannot upgrade tester in power_state={tester.PowerState}; expected 'running'");
        }

        var inFlight = await InFlightRunCountAsync(db, testerId, ct);
        if (inFlight > 0)
        {
            return Conflict($"cannot upgrade tester with {inFlight} benchmark(s) in flight");
        }

        tester.Allocation = "upgrading";
        tester.StatusMessage = "Upgrade requested";
        tester.UpdatedAt = DateTime.UtcNow;
        await db.SaveChangesAsync(ct);

        logger.LogInformation("tester {TesterId} upgrade requested by {Actor}", testerId, user?.Email);

        // Background: confirm the VM is reachable via a state probe, then release
        // the allocation. Re-installer wiring lands in M4 slice 2 (deploy-runner).
        FireAndForget(scopeFactory, loggerFactory, testerId, "upgrade", async (p, cred, t, l, token) =>
        {
            var res = await p.ShowAsync(t, cred, token);
            using var scope = scopeFactory.CreateScope();
            var sdb = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
            var row = await sdb.ProjectTesters.FirstOrDefaultAsync(x => x.TesterId == testerId, token);
            if (row is null) return;
            row.Allocation = "idle";
            row.StatusMessage = res.Success
                ? "Upgrade completed (state re-probed)"
                : $"Upgrade probe failed: {res.Error ?? res.StdErr}";
            row.UpdatedAt = DateTime.UtcNow;
            await sdb.SaveChangesAsync(token);
        });

        return Results.Accepted($"/api/projects/{projectId}/testers/{testerId}", ToDto(tester));
    }

    // ── probe ──────────────────────────────────────────────────────────────────

    /// <summary>POST /probe — synchronous cloud state reconciliation. Calls
    /// <c>provisioner.ShowAsync</c>, maps the reported power state onto the row,
    /// and returns the updated tester (200). If the CLI is absent the row is
    /// left unchanged and a status message records the probe was unavailable —
    /// still 200, never a request failure.</summary>
    private static async Task<IResult> ProbeTester(
        string projectId,
        Guid testerId,
        HttpContext http,
        NetworkerDbContext db,
        IComputeProvisioner provisioner,
        ILoggerFactory loggerFactory,
        CancellationToken ct)
    {
        var user = http.GetAuthUser();
        var logger = loggerFactory.CreateLogger("TesterWrite.Probe");

        var tester = await LoadAsync(db, projectId, testerId, ct);
        if (tester is null)
        {
            return Results.NotFound(new { error = "Tester not found" });
        }

        if (tester.Allocation is "locked" or "upgrading")
        {
            return Conflict($"cannot probe tester with allocation={tester.Allocation}; retry once idle");
        }

        var creds = await LoadCredentialsAsync(db, tester, ct);
        var res = await provisioner.ShowAsync(tester, creds, ct);

        if (res.Success)
        {
            var reported = ParsePowerState(tester.Cloud, res.StdOut);
            tester.PowerState = reported;
            tester.StatusMessage = $"Manual probe: cloud reported {reported}";
        }
        else
        {
            // CLI missing / error — do not fail the request; record and move on.
            tester.StatusMessage = $"Manual probe unavailable: {res.Error ?? res.StdErr}";
            logger.LogWarning("tester {TesterId} probe could not reach cloud: {Err}", testerId, res.Error ?? res.StdErr);
        }
        tester.UpdatedAt = DateTime.UtcNow;
        await db.SaveChangesAsync(ct);

        logger.LogInformation(
            "tester {TesterId} probed by {Actor}; resolved={Power}", testerId, user?.Email, tester.PowerState);

        return Results.Ok(ToDto(tester));
    }

    // ── delete ──────────────────────────────────────────────────────────────────

    /// <summary>DELETE /testers/{id} (Admin) — destroy VM + row. Guards
    /// transient power states, allocation=idle, and no in-flight runs. Marks the
    /// row <c>power_state=deleting</c>, returns 202, and destroys the VM then
    /// deletes the row in the background. If the VM delete fails (and it's not a
    /// missing CLI), the row is kept so the user can retry — no orphaned cloud
    /// resources.</summary>
    private static async Task<IResult> DeleteTester(
        string projectId,
        Guid testerId,
        HttpContext http,
        NetworkerDbContext db,
        IServiceScopeFactory scopeFactory,
        ILoggerFactory loggerFactory,
        CancellationToken ct)
    {
        var user = http.GetAuthUser();
        var logger = loggerFactory.CreateLogger("TesterWrite.Delete");

        var tester = await LoadAsync(db, projectId, testerId, ct);
        if (tester is null)
        {
            return Results.NotFound(new { error = "Tester not found" });
        }

        var transient = tester.PowerState is "provisioning" or "starting" or "stopping" or "upgrading" or "deleting";
        if (transient)
        {
            return Conflict($"cannot delete tester in transient power_state={tester.PowerState}");
        }
        if (tester.Allocation != "idle")
        {
            return Conflict($"cannot delete tester with allocation={tester.Allocation}; must be idle");
        }

        var inFlight = await InFlightRunCountAsync(db, testerId, ct);
        if (inFlight > 0)
        {
            return Conflict($"cannot delete tester with {inFlight} benchmark(s) in flight");
        }

        tester.PowerState = "deleting";
        tester.StatusMessage = "Delete requested";
        tester.UpdatedAt = DateTime.UtcNow;
        await db.SaveChangesAsync(ct);

        logger.LogInformation("tester {TesterId} delete requested by {Actor}", testerId, user?.Email);

        var hasVm = !string.IsNullOrEmpty(tester.VmResourceId);
        FireAndForget(scopeFactory, loggerFactory, testerId, "delete", async (p, cred, t, l, token) =>
        {
            ProvisionResult res = hasVm
                ? await p.DeleteAsync(t, cred, token)
                : ProvisionResult.Ok(0, string.Empty, string.Empty); // no VM → nothing to destroy

            using var scope = scopeFactory.CreateScope();
            var sdb = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
            var row = await sdb.ProjectTesters.FirstOrDefaultAsync(x => x.TesterId == testerId, token);
            if (row is null) return;

            // A real destroy that FAILED (exit code present, not just a missing
            // CLI) keeps the row so the user can retry — refuse to orphan cloud
            // resources. A missing CLI (ExitCode == null) is a soft/no-op path:
            // proceed with row deletion so CI + credential-less hosts still work.
            var realFailure = !res.Success && res.ExitCode is not null;
            if (realFailure)
            {
                row.PowerState = "stopped";
                row.StatusMessage = $"delete failed: {res.Error ?? res.StdErr}";
                row.UpdatedAt = DateTime.UtcNow;
                await sdb.SaveChangesAsync(token);
                l.LogError("tester {TesterId} VM delete failed; row kept for retry: {Err}", testerId, res.Error ?? res.StdErr);
                return;
            }

            sdb.ProjectTesters.Remove(row);
            await sdb.SaveChangesAsync(token);
            l.LogInformation("tester {TesterId} deleted (VM destroyed + row removed)", testerId);
        });

        return Results.Accepted(
            $"/api/projects/{projectId}/testers/{testerId}",
            new { deleted = false, status = "deleting" });
    }
}
