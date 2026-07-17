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
}
