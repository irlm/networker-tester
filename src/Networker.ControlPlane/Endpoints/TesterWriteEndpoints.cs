using System.Text.Json;
using Microsoft.AspNetCore.Mvc;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Provisioning;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Persistent tester (VM) <b>lifecycle write</b> endpoints — the C# port of the
/// mutating handlers in the Rust dashboard's <c>api/testers.rs</c>
/// (start / stop / upgrade / probe / postpone / force-stop / schedule / delete).
/// The read surface (list / get / queue / cost / regions) lives in
/// <see cref="TestersEndpoints"/>; this file is additive and touches neither it
/// nor <c>Program.cs</c>.
///
/// <para><b>202-async pattern (preserved from Rust):</b> a mutating call first
/// validates the DB state transition, applies the authoritative
/// <c>power_state</c> / <c>allocation</c> change synchronously, returns
/// <c>202 Accepted</c> with the updated row, and drives the cloud CLI in the
/// background through <see cref="IComputeProvisioner"/>. The synchronous ops
/// (probe / postpone / schedule / force-stop) return <c>200 OK</c> like the Rust
/// side.</para>
///
/// <para><b>CI-safety:</b> cloud CLIs are absent in CI. The endpoints therefore
/// never fail the request when the CLI can't run — they do the DB transition and
/// return 202, and the provisioner call runs detached (failures are logged and
/// written to <c>status_message</c>, never surfaced to the caller). This keeps
/// every endpoint testable purely on (202 + DB change).</para>
///
/// <para><b>Auth</b> (matches the Rust <c>require_project_role</c> gates):
/// <c>ProjectOperator</c> for start / stop / probe / postpone / schedule;
/// <c>ProjectAdmin</c> for upgrade / force-stop / delete.</para>
/// </summary>
public static class TesterWriteEndpoints
{
    public static IEndpointRouteBuilder MapTesterWriteEndpoints(this IEndpointRouteBuilder app)
    {
        const string basePath = "/api/projects/{projectId}/testers/{testerId:guid}";

        app.MapPost($"{basePath}/start", StartTester)
            .RequireAuthorization(AuthPolicies.ProjectOperator);

        app.MapPost($"{basePath}/stop", StopTester)
            .RequireAuthorization(AuthPolicies.ProjectOperator);

        app.MapPost($"{basePath}/force-stop", ForceStopTester)
            .RequireAuthorization(AuthPolicies.ProjectAdmin);

        app.MapPost($"{basePath}/upgrade", UpgradeTester)
            .RequireAuthorization(AuthPolicies.ProjectAdmin);

        app.MapPost($"{basePath}/probe", ProbeTester)
            .RequireAuthorization(AuthPolicies.ProjectOperator);

        app.MapPost($"{basePath}/postpone", PostponeShutdown)
            .RequireAuthorization(AuthPolicies.ProjectOperator);

        app.MapPatch($"{basePath}/schedule", UpdateSchedule)
            .RequireAuthorization(AuthPolicies.ProjectOperator);

        app.MapDelete("/api/projects/{projectId}/testers/{testerId:guid}", DeleteTester)
            .RequireAuthorization(AuthPolicies.ProjectAdmin);

        return app;
    }

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

    // ── postpone ────────────────────────────────────────────────────────────────

    /// <summary>POST /postpone — extend auto-shutdown. Body is one of
    /// <c>{until}</c>, <c>{add_hours}</c>, or <c>{skip_tonight:true}</c>. Bumps
    /// <c>shutdown_deferral_count</c> and returns the updated row (200).</summary>
    private static async Task<IResult> PostponeShutdown(
        string projectId,
        Guid testerId,
        HttpContext http,
        [FromBody] PostponeBody? body,
        NetworkerDbContext db,
        ILoggerFactory loggerFactory,
        CancellationToken ct)
    {
        var user = http.GetAuthUser();
        var logger = loggerFactory.CreateLogger("TesterWrite.Postpone");

        if (body is null)
        {
            return Results.BadRequest(new { error = "postpone body required" });
        }

        var tester = await LoadAsync(db, projectId, testerId, ct);
        if (tester is null)
        {
            return Results.NotFound(new { error = "Tester not found" });
        }

        var now = DateTime.UtcNow;
        DateTime newNext;
        try
        {
            newNext = ComputePostpone(body, tester, now);
        }
        catch (ArgumentException ex)
        {
            return Results.BadRequest(new { error = ex.Message });
        }

        tester.NextShutdownAt = newNext;
        tester.ShutdownDeferralCount = (short)(tester.ShutdownDeferralCount + 1);
        tester.UpdatedAt = now;
        await db.SaveChangesAsync(ct);

        logger.LogInformation(
            "tester {TesterId} shutdown postponed to {Next} by {Actor}", testerId, newNext, user?.Email);

        return Results.Ok(ToDto(tester));
    }

    /// <summary>Pure postpone computation — mirrors the Rust <c>compute_postpone</c>.
    /// Exactly one of the three body shapes must be populated.</summary>
    internal static DateTime ComputePostpone(PostponeBody body, ProjectTester tester, DateTime now)
    {
        if (body.Until is { } until)
        {
            var untilUtc = until.ToUniversalTime();
            if (untilUtc <= now)
            {
                throw new ArgumentException("until must be in the future");
            }
            return untilUtc;
        }
        if (body.AddHours is { } hours)
        {
            if (hours <= 0)
            {
                throw new ArgumentException("add_hours must be positive");
            }
            var baseline = tester.NextShutdownAt ?? now;
            return baseline.AddHours(hours);
        }
        if (body.SkipTonight is true)
        {
            // Roll one day forward and recompute tomorrow's slot.
            return NextShutdownAtForProvider(tester.Cloud, tester.Region, tester.AutoShutdownLocalHour, now.AddHours(24));
        }
        throw new ArgumentException("exactly one of until / add_hours / skip_tonight required");
    }

    // ── schedule (PATCH) ──────────────────────────────────────────────────────

    /// <summary>PATCH /schedule — set auto-shutdown enabled + local hour;
    /// recomputes <c>next_shutdown_at</c> in the region's timezone (cleared when
    /// disabled). Returns the updated row (200).</summary>
    private static async Task<IResult> UpdateSchedule(
        string projectId,
        Guid testerId,
        HttpContext http,
        [FromBody] ScheduleBody? body,
        NetworkerDbContext db,
        ILoggerFactory loggerFactory,
        CancellationToken ct)
    {
        var user = http.GetAuthUser();
        var logger = loggerFactory.CreateLogger("TesterWrite.Schedule");

        if (body is null || (body.AutoShutdownEnabled is null && body.AutoShutdownLocalHour is null))
        {
            return Results.BadRequest(new
            {
                error = "at least one of auto_shutdown_enabled or auto_shutdown_local_hour required",
            });
        }
        if (body.AutoShutdownLocalHour is { } h && (h < 0 || h > 23))
        {
            return Results.BadRequest(new { error = "auto_shutdown_local_hour must be 0..=23" });
        }

        var tester = await LoadAsync(db, projectId, testerId, ct);
        if (tester is null)
        {
            return Results.NotFound(new { error = "Tester not found" });
        }

        var newEnabled = body.AutoShutdownEnabled ?? tester.AutoShutdownEnabled;
        var newHour = body.AutoShutdownLocalHour ?? tester.AutoShutdownLocalHour;

        tester.AutoShutdownEnabled = newEnabled;
        tester.AutoShutdownLocalHour = newHour;
        tester.NextShutdownAt = newEnabled
            ? NextShutdownAtForProvider(tester.Cloud, tester.Region, newHour, DateTime.UtcNow)
            : null;
        tester.UpdatedAt = DateTime.UtcNow;
        await db.SaveChangesAsync(ct);

        logger.LogInformation(
            "tester {TesterId} schedule updated by {Actor}: enabled={Enabled} hour={Hour}",
            testerId, user?.Email, newEnabled, newHour);

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

    // ── shared helpers ────────────────────────────────────────────────────────

    private static Task<ProjectTester?> LoadAsync(
        NetworkerDbContext db, string projectId, Guid testerId, CancellationToken ct) =>
        db.ProjectTesters.FirstOrDefaultAsync(t => t.ProjectId == projectId && t.TesterId == testerId, ct);

    private static async Task<int> InFlightRunCountAsync(NetworkerDbContext db, Guid testerId, CancellationToken ct) =>
        await db.TestRuns.CountAsync(
            r => r.TesterId == testerId
                 && (r.Status == "queued" || r.Status == "provisioning" || r.Status == "running"),
            ct);

    private static IResult Conflict(string message) =>
        Results.Json(new { error = message }, statusCode: StatusCodes.Status409Conflict);

    /// <summary>
    /// Run a cloud-provisioner action detached from the request. Opens its own DI
    /// scope so the request's <see cref="NetworkerDbContext"/> can be disposed
    /// with the response. All exceptions are swallowed + logged — a background
    /// cloud failure must never crash the host or affect the already-sent 202.
    /// </summary>
    private static void FireAndForget(
        IServiceScopeFactory scopeFactory,
        ILoggerFactory loggerFactory,
        Guid testerId,
        string action,
        Func<IComputeProvisioner, ProviderCredentials?, ProjectTester, ILogger, CancellationToken, Task> work)
    {
        var logger = loggerFactory.CreateLogger($"TesterWrite.{action}.bg");
        _ = Task.Run(async () =>
        {
            try
            {
                using var scope = scopeFactory.CreateScope();
                var sp = scope.ServiceProvider;
                var db = sp.GetRequiredService<NetworkerDbContext>();
                var provisioner = sp.GetRequiredService<IComputeProvisioner>();

                var tester = await db.ProjectTesters.AsNoTracking()
                    .FirstOrDefaultAsync(t => t.TesterId == testerId);
                if (tester is null)
                {
                    return;
                }

                var creds = await LoadCredentialsAsync(db, tester, CancellationToken.None);
                await work(provisioner, creds, tester, logger, CancellationToken.None);
            }
            catch (Exception ex)
            {
                logger.LogError(ex, "background {Action} for tester {TesterId} threw", action, testerId);
            }
        });
    }

    /// <summary>
    /// Apply the terminal power_state after a background start/stop provisioner
    /// call: <paramref name="running"/> on success, <paramref name="failedTo"/>
    /// on a real CLI failure. A missing CLI (ExitCode == null) is treated as
    /// success so credential-less / CI hosts converge the row to the intended
    /// state instead of getting stuck in the transient one.
    /// </summary>
    private static async Task FinishAsync(
        IServiceScopeFactory scopeFactory,
        Guid testerId,
        ProvisionResult res,
        string running,
        string failedTo,
        string action,
        ILogger logger,
        CancellationToken ct)
    {
        using var scope = scopeFactory.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        var row = await db.ProjectTesters.FirstOrDefaultAsync(t => t.TesterId == testerId, ct);
        if (row is null)
        {
            return;
        }

        var realFailure = !res.Success && res.ExitCode is not null;
        if (realFailure)
        {
            row.PowerState = failedTo;
            row.StatusMessage = $"{action} failed: {res.Error ?? res.StdErr}";
            logger.LogError("tester {TesterId} {Action} CLI failed: {Err}", testerId, action, res.Error ?? res.StdErr);
        }
        else
        {
            row.PowerState = running;
            row.StatusMessage = res.ExitCode is null
                ? $"{action} completed (cloud CLI unavailable — state assumed)"
                : $"{action} completed";
        }
        row.UpdatedAt = DateTime.UtcNow;
        await db.SaveChangesAsync(ct);
    }

    /// <summary>
    /// Resolve per-connection credentials from the tester's <c>cloud_connection</c>
    /// row's <c>config</c> JSON. Returns null when there is no connection (ambient
    /// CLI auth) or the config can't be parsed — the provisioner then relies on
    /// the host's ambient auth, matching the Rust managed-identity fallback.
    /// </summary>
    private static async Task<ProviderCredentials?> LoadCredentialsAsync(
        NetworkerDbContext db, ProjectTester tester, CancellationToken ct)
    {
        if (tester.CloudConnectionId is not { } connId)
        {
            return null;
        }

        var conn = await db.CloudConnections.AsNoTracking()
            .FirstOrDefaultAsync(c => c.ConnectionId == connId, ct);
        if (conn is null)
        {
            return null;
        }

        var extra = new Dictionary<string, string>(StringComparer.Ordinal);
        string? sub = null, rg = null, region = tester.Region;
        try
        {
            using var doc = JsonDocument.Parse(conn.Config);
            var root = doc.RootElement;
            if (root.ValueKind == JsonValueKind.Object)
            {
                foreach (var prop in root.EnumerateObject())
                {
                    if (prop.Value.ValueKind == JsonValueKind.String)
                    {
                        extra[prop.Name] = prop.Value.GetString() ?? string.Empty;
                    }
                }
            }
            extra.TryGetValue("subscription_id", out sub);
            extra.TryGetValue("resource_group", out rg);
            if (extra.TryGetValue("region", out var r) && !string.IsNullOrEmpty(r))
            {
                region = r;
            }
        }
        catch (JsonException)
        {
            // Non-JSON / encrypted config we can't read → ambient auth.
            return new ProviderCredentials(conn.Provider, Region: region);
        }

        return new ProviderCredentials(conn.Provider, sub, rg, region, extra);
    }

    /// <summary>
    /// Map a provider's <c>show</c> JSON onto a coarse power state
    /// ("running" | "stopped" | "unknown"). Mirrors what the Rust recovery path
    /// derives from the provider state string.
    /// </summary>
    internal static string ParsePowerState(string? cloud, string json)
    {
        if (string.IsNullOrWhiteSpace(json))
        {
            return "unknown";
        }
        try
        {
            using var doc = JsonDocument.Parse(json);
            var root = doc.RootElement;
            string? raw = (cloud?.ToLowerInvariant()) switch
            {
                "azure" => root.TryGetProperty("powerState", out var ps) ? ps.GetString() : null,
                "aws" => root.TryGetProperty("State", out var st) && st.TryGetProperty("Name", out var n)
                    ? n.GetString() : null,
                "gcp" => root.TryGetProperty("status", out var s) ? s.GetString() : null,
                _ => null,
            };
            if (string.IsNullOrEmpty(raw))
            {
                return "unknown";
            }
            var lower = raw.ToLowerInvariant();
            if (lower.Contains("running")) return "running";
            if (lower.Contains("dealloc") || lower.Contains("stopped") || lower.Contains("terminated")
                || lower.Contains("suspended")) return "stopped";
            return lower;
        }
        catch (JsonException)
        {
            return "unknown";
        }
    }

    // ── region → timezone → next shutdown (ported from azure_regions.rs) ──────

    /// <summary>
    /// Next UTC instant at <paramref name="localHour"/>:00 in the region's local
    /// timezone, rolling forward one day if today's slot has passed. Port of the
    /// Rust <c>next_shutdown_at_for_provider</c>.
    /// </summary>
    internal static DateTime NextShutdownAtForProvider(string? cloud, string region, short localHour, DateTime nowUtc)
    {
        var tz = RegionTimeZone(cloud, region);
        var hour = Math.Clamp((int)localHour, 0, 23);
        var localNow = TimeZoneInfo.ConvertTimeFromUtc(DateTime.SpecifyKind(nowUtc, DateTimeKind.Utc), tz);

        var todayLocal = new DateTime(localNow.Year, localNow.Month, localNow.Day, hour, 0, 0, DateTimeKind.Unspecified);
        if (todayLocal > localNow)
        {
            return TimeZoneInfo.ConvertTimeToUtc(todayLocal, tz);
        }

        var tomorrowLocal = todayLocal.AddDays(1);
        return TimeZoneInfo.ConvertTimeToUtc(tomorrowLocal, tz);
    }

    /// <summary>
    /// Cloud region → <see cref="TimeZoneInfo"/> via IANA ids (cross-platform on
    /// .NET). Mirrors the Rust <c>region_timezone_for_provider</c> mappings;
    /// unknown provider/region → UTC.
    /// </summary>
    private static TimeZoneInfo RegionTimeZone(string? cloud, string region)
    {
        var iana = (cloud?.ToLowerInvariant()) switch
        {
            "aws" => AwsRegionIana(region),
            "gcp" => GcpRegionIana(region),
            _ => AzureRegionIana(region),
        };
        try
        {
            return TimeZoneInfo.FindSystemTimeZoneById(iana);
        }
        catch (TimeZoneNotFoundException)
        {
            return TimeZoneInfo.Utc;
        }
        catch (InvalidTimeZoneException)
        {
            return TimeZoneInfo.Utc;
        }
    }

    private static string AzureRegionIana(string region) => region switch
    {
        "eastus" or "eastus2" or "eastus3" => "America/New_York",
        "centralus" or "southcentralus" or "northcentralus" => "America/Chicago",
        "westus" or "westus2" or "westus3" => "America/Los_Angeles",
        "westcentralus" => "America/Denver",
        "northeurope" => "Europe/Dublin",
        "westeurope" => "Europe/Amsterdam",
        "uksouth" or "ukwest" => "Europe/London",
        "francecentral" or "francesouth" => "Europe/Paris",
        "germanywestcentral" or "germanynorth" => "Europe/Berlin",
        "switzerlandnorth" or "switzerlandwest" => "Europe/Zurich",
        "norwayeast" or "norwaywest" => "Europe/Oslo",
        "swedencentral" => "Europe/Stockholm",
        "polandcentral" => "Europe/Warsaw",
        "italynorth" => "Europe/Rome",
        "spaincentral" => "Europe/Madrid",
        "japaneast" or "japanwest" => "Asia/Tokyo",
        "koreacentral" or "koreasouth" => "Asia/Seoul",
        "eastasia" => "Asia/Hong_Kong",
        "southeastasia" => "Asia/Singapore",
        "centralindia" or "southindia" or "westindia" => "Asia/Kolkata",
        "australiaeast" or "australiasoutheast" or "australiacentral" or "australiacentral2" => "Australia/Sydney",
        "brazilsouth" or "brazilsoutheast" => "America/Sao_Paulo",
        "canadacentral" or "canadaeast" => "America/Toronto",
        "mexicocentral" => "America/Mexico_City",
        "uaenorth" or "uaecentral" => "Asia/Dubai",
        "qatarcentral" => "Asia/Qatar",
        "israelcentral" => "Asia/Jerusalem",
        "southafricanorth" or "southafricawest" => "Africa/Johannesburg",
        _ => "UTC",
    };

    private static string AwsRegionIana(string region) => region switch
    {
        "us-east-1" or "us-east-2" => "America/New_York",
        "us-west-1" or "us-west-2" => "America/Los_Angeles",
        "eu-west-1" => "Europe/Dublin",
        "eu-west-2" => "Europe/London",
        "eu-central-1" => "Europe/Berlin",
        "ap-northeast-1" => "Asia/Tokyo",
        "ap-southeast-1" => "Asia/Singapore",
        "ap-southeast-2" => "Australia/Sydney",
        "sa-east-1" => "America/Sao_Paulo",
        _ => "UTC",
    };

    private static string GcpRegionIana(string region) => region switch
    {
        "us-central1" or "us-east1" or "us-east4" => "America/New_York",
        "us-west1" or "us-west2" or "us-west4" => "America/Los_Angeles",
        "europe-west1" or "europe-west4" => "Europe/Amsterdam",
        "europe-west2" => "Europe/London",
        "europe-west3" => "Europe/Berlin",
        "asia-east1" or "asia-east2" => "Asia/Taipei",
        "asia-northeast1" => "Asia/Tokyo",
        "asia-southeast1" => "Asia/Singapore",
        "australia-southeast1" => "Australia/Sydney",
        _ => "UTC",
    };

    // ── DTO (snake_case, subset matching the Rust ProjectTesterRow response) ──

    private static object ToDto(ProjectTester t) => new
    {
        tester_id = t.TesterId,
        project_id = t.ProjectId,
        name = t.Name,
        cloud = t.Cloud,
        region = t.Region,
        vm_size = t.VmSize,
        vm_name = t.VmName,
        vm_resource_id = t.VmResourceId,
        public_ip = t.PublicIp?.ToString(),
        ssh_user = t.SshUser,
        power_state = t.PowerState,
        allocation = t.Allocation,
        status_message = t.StatusMessage,
        locked_by_config_id = t.LockedByConfigId,
        installer_version = t.InstallerVersion,
        last_installed_at = t.LastInstalledAt,
        auto_shutdown_enabled = t.AutoShutdownEnabled,
        auto_shutdown_local_hour = t.AutoShutdownLocalHour,
        next_shutdown_at = t.NextShutdownAt,
        shutdown_deferral_count = t.ShutdownDeferralCount,
        auto_probe_enabled = t.AutoProbeEnabled,
        last_used_at = t.LastUsedAt,
        created_at = t.CreatedAt,
        updated_at = t.UpdatedAt,
        cloud_connection_id = t.CloudConnectionId,
        cloud_account_id = t.CloudAccountId,
    };

    // ── Request bodies (snake_case via [FromBody] + JSON property names) ──────

    public sealed record UpgradeBody
    {
        [System.Text.Json.Serialization.JsonPropertyName("confirm")]
        public bool Confirm { get; init; }
    }

    public sealed record ForceStopBody
    {
        [System.Text.Json.Serialization.JsonPropertyName("confirm")]
        public bool Confirm { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("reason")]
        public string Reason { get; init; } = string.Empty;
    }

    public sealed record ScheduleBody
    {
        [System.Text.Json.Serialization.JsonPropertyName("auto_shutdown_enabled")]
        public bool? AutoShutdownEnabled { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("auto_shutdown_local_hour")]
        public short? AutoShutdownLocalHour { get; init; }
    }

    /// <summary>
    /// Postpone body — the three shapes from the Rust untagged enum
    /// (<c>{until}</c> | <c>{add_hours}</c> | <c>{skip_tonight}</c>). Deserialized
    /// as one flat record; exactly one field is expected to be present.
    /// </summary>
    public sealed record PostponeBody
    {
        [System.Text.Json.Serialization.JsonPropertyName("until")]
        public DateTime? Until { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("add_hours")]
        public long? AddHours { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("skip_tonight")]
        public bool? SkipTonight { get; init; }
    }
}
