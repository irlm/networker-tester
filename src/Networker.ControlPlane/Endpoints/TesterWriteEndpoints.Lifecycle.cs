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
            return ApiError.NotFound("Tester not found");
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
            return ApiError.NotFound("Tester not found");
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
            return ApiError.BadRequest("force-stop requires {\"confirm\": true, \"reason\": \"...\"}");
        }
        if (string.IsNullOrWhiteSpace(body.Reason))
        {
            return ApiError.BadRequest("reason must not be empty");
        }

        var tester = await LoadAsync(db, projectId, testerId, ct);
        if (tester is null)
        {
            return ApiError.NotFound("Tester not found");
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

    /// <summary>POST /upgrade (Admin) — HONEST 501 (fidelity audit F23).
    /// The Rust dashboard re-installed the tester binaries over SSH
    /// (<c>services/tester_install.rs::install_tester</c>); that path has not
    /// been ported. The previous C# behaviour marked the row
    /// <c>upgrading</c>, ran a cloud state probe, and wrote "Upgrade
    /// completed (state re-probed)" — a silent lie that left testers on old
    /// versions while the UI reported success. Until the SSH re-install (or
    /// an agent self-update command) is wired, this refuses loudly and
    /// mutates nothing. Request validation (400/404) is kept so the route's
    /// contract stays testable.</summary>
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
            return ApiError.BadRequest("upgrade requires {\"confirm\": true}");
        }

        var tester = await LoadAsync(db, projectId, testerId, ct);
        if (tester is null)
        {
            return ApiError.NotFound("Tester not found");
        }

        logger.LogWarning(
            "tester {TesterId} upgrade requested by {Actor} — refused with 501: SSH re-install not ported",
            testerId, user?.Email);

        return ApiError.Status(
            StatusCodes.Status501NotImplemented,
            "tester upgrade (SSH re-install) is not implemented in the C# control plane yet — "
            + "no binaries were changed; tracked in the fidelity audit (F23). "
            + "Workaround: delete and re-deploy the runner to get current binaries.");
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
            return ApiError.NotFound("Tester not found");
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
        // ?force=true removes the tester record even if the cloud VM delete fails
        // (e.g. the cloud creds are gone / unreachable, so the VM can't be
        // verified or destroyed). The escape hatch for a tester that would
        // otherwise be un-deletable — at the cost of possibly orphaning a VM the
        // operator must clean up in their cloud console. Bound from the query
        // string (absent → false).
        bool force,
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
            return ApiError.NotFound("Tester not found");
        }

        // A tester genuinely mid-operation updates UpdatedAt as it progresses, so
        // block the delete only while that transient state is RECENT. A tester
        // WEDGED in a transient state (e.g. a failed auto-shutdown that stuck it in
        // "stopping" long ago) must stay deletable — otherwise it can never be
        // removed ("delete but doesn't delete"). 15 min comfortably exceeds any
        // real transition.
        var transient = tester.PowerState is "provisioning" or "starting" or "stopping" or "upgrading" or "deleting";
        var recentlyActive = (DateTime.UtcNow - tester.UpdatedAt) < TimeSpan.FromMinutes(15);
        if (transient && recentlyActive)
        {
            return Conflict(
                $"cannot delete tester in transient power_state={tester.PowerState}; retry once it settles");
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
            //
            // EXCEPTION: a cloud "resource not found / already gone" error is the
            // DESIRED end state (no VM), not a failure — otherwise a tester whose
            // VM was deleted out-of-band can never be removed ("delete but doesn't
            // delete"). Treat it as success and remove the row.
            var realFailure = !res.Success && res.ExitCode is not null && !VmAlreadyGone(res);
            if (realFailure && !force)
            {
                row.PowerState = "stopped";
                row.StatusMessage = $"delete failed: {res.Error ?? res.StdErr}";
                row.UpdatedAt = DateTime.UtcNow;
                await sdb.SaveChangesAsync(token);
                l.LogError("tester {TesterId} VM delete failed; row kept for retry: {Err}", testerId, res.Error ?? res.StdErr);
                return;
            }

            if (realFailure)
            {
                // force=true — the cloud delete genuinely failed (e.g. dead creds),
                // but the operator chose to remove the record anyway. The VM may be
                // orphaned and must be cleaned up in the cloud console.
                l.LogWarning(
                    "tester {TesterId} FORCE-deleted despite cloud VM delete failure — the VM may be orphaned "
                    + "and must be cleaned up in the cloud console: {Err}", testerId, res.Error ?? res.StdErr);
            }

            sdb.ProjectTesters.Remove(row);
            await sdb.SaveChangesAsync(token);
            if (!res.Success && res.ExitCode is not null)
            {
                l.LogInformation(
                    "tester {TesterId} deleted (VM was already gone — cloud reported not-found; row removed)", testerId);
            }
            else
            {
                l.LogInformation("tester {TesterId} deleted (VM destroyed + row removed)", testerId);
            }
        });

        return Results.Accepted(
            $"/api/projects/{projectId}/testers/{testerId}",
            new { deleted = false, status = "deleting" });
    }

    /// <summary>
    /// True when a failed cloud delete actually means "the VM is already gone" —
    /// a delete of a non-existent resource is the DESIRED end state, not a failure
    /// that should keep the tester row. Covers the not-found signals of az / aws /
    /// gcloud (e.g. Azure <c>ResourceNotFound</c>, AWS <c>InvalidInstanceID.NotFound</c>,
    /// GCP <c>was not found</c>).
    /// </summary>
    internal static bool VmAlreadyGone(ProvisionResult res)
    {
        if (res.Success)
        {
            return false;
        }
        var text = $"{res.Error} {res.StdErr}".ToLowerInvariant();
        return text.Contains("not found")
            || text.Contains("notfound")
            || text.Contains("does not exist")
            || text.Contains("could not be found")
            || text.Contains("was not found")
            || text.Contains("no longer exists")
            || text.Contains("resourcenotfound");
    }
}
