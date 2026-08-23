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

// Background-provisioner plumbing shared by the lifecycle handlers
// (FireAndForget / FinishAsync / credential resolution / power-state parsing)
// for TesterWriteEndpoints (route mapping lives in TesterWriteEndpoints.cs).
public static partial class TesterWriteEndpoints
{
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

                var creds = await LoadCredentialsAsync(sp, db, tester, logger, CancellationToken.None);
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
    /// Credentials for a tester lifecycle CLI call. Delegates to
    /// <see cref="TesterLifecycleCredentials"/> — the ONE resolver shared with
    /// the auto-shutdown/auto-wake sweep and the agent auto-upgrade, so a GCP
    /// tester's service-account key is threaded in here too and a manual
    /// stop/start/delete/probe does not hit "You do not currently have an
    /// active account selected" (#857).
    /// </summary>
    /// <param name="services">The scope the call runs in; the credential cipher
    /// is optional (a host may register none), hence GetService.</param>
    private static Task<ProviderCredentials?> LoadCredentialsAsync(
        IServiceProvider services,
        NetworkerDbContext db,
        ProjectTester tester,
        ILogger logger,
        CancellationToken ct) =>
        TesterLifecycleCredentials.LoadAsync(
            db, services.GetService<CredentialCipher>(), tester, logger, ct);

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
                // DockerComputeProvisioner.NormalizedShowJson: {"powerState": running|stopped}
                "docker" => root.TryGetProperty("powerState", out var dps) ? dps.GetString() : null,
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
}
