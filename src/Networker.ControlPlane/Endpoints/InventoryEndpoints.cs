using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Provisioning;
using Networker.ControlPlane.Security;
using Networker.Data;
using Networker.Security;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Cloud resource scan — what the Settings page's "scan all providers" button
/// calls.
///
/// <para>Route: <b>GET /api/projects/{projectId}/inventory</b> → response
/// <c>{ vms: [CloudVm...], errors: [string...], scanned: [string...],
/// not_configured: [string...], scanned_at: RFC3339 }</c>. Each <c>CloudVm</c>
/// is snake_case: provider, name, region, status, public_ip, fqdn, vm_size, os,
/// resource_group, managed.</para>
///
/// <para><b>History:</b> the phase-3 port shipped this as a stub that always
/// returned <c>{vms: [], errors: []}</c>, on the (then-true) grounds that the
/// <c>az</c>/<c>aws</c>/<c>gcloud</c> CLIs were not available to the C# control
/// plane. They are: the control plane provisions and tears down production VMs
/// through them (<see cref="CliComputeProvisioner"/>,
/// <see cref="Background.OrphanReaperService"/>,
/// <see cref="ProviderCredentialValidator"/>). The scan is now real —
/// <see cref="CloudInventoryScanner"/> enumerates each of the project's cloud
/// accounts in parallel with that account's own stored credentials — and the
/// response says what it scanned so an empty table can never again look like a
/// dead button.</para>
///
/// <para><b>Read-only:</b> every command the scanner runs is an enumeration
/// (plus the Azure sign-in that authenticates one). This endpoint is open to any
/// project MEMBER and must never gain a mutating path.</para>
///
/// <para><b>What counts as an error:</b> a provider the project has no cloud
/// account for is NOT an error — nothing failed, and yellow text for a
/// deliberate absence trains people to ignore yellow text. It is reported as
/// <c>not_configured</c> instead, and the UI names it. A provider that IS
/// configured but could not be enumerated (inactive account, undecryptable
/// credentials, missing CLI, failed sign-in, non-zero exit, timeout) always
/// contributes an <c>errors[]</c> line naming the provider and the account.</para>
/// </summary>
public static class InventoryEndpoints
{
    /// <summary>
    /// Ceiling for the whole scan, whatever the per-account budgets add up to:
    /// the HTTP request must always come back with a rendered answer (possibly
    /// "these providers timed out"), never hang the Settings page.
    /// </summary>
    internal static TimeSpan RequestBudget { get; set; } = TimeSpan.FromSeconds(150);

    public static IEndpointRouteBuilder MapInventoryEndpoints(this IEndpointRouteBuilder app)
    {
        app.MapGet("/api/projects/{projectId}/inventory", async (
            string projectId,
            NetworkerDbContext db,
            CloudInventoryScanner scanner,
            IServiceProvider services,
            ILoggerFactory lf,
            CancellationToken ct) =>
        {
            var logger = lf.CreateLogger("Networker.Inventory");

            // Managed deployment IPs for cross-referencing (list_all: newest 100).
            var managedHosts = await GetManagedHostsAsync(db, ct).ConfigureAwait(false);

            var (accounts, errors, configured) = await LoadAccountsAsync(
                db, services.GetService<CredentialCipher>(), projectId, logger, ct).ConfigureAwait(false);

            using var budget = CancellationTokenSource.CreateLinkedTokenSource(ct);
            budget.CancelAfter(RequestBudget);
            var scan = await scanner.ScanAsync(accounts, managedHosts, budget.Token).ConfigureAwait(false);

            errors.AddRange(scan.Errors);
            var notConfigured = CloudInventoryScanner.KnownProviders
                .Where(p => !configured.Contains(p))
                .ToList();

            logger.LogInformation(
                "Inventory scan (project {ProjectId}): {Accounts} account(s) across [{Scanned}], "
                + "{Vms} VM(s), {Errors} error(s); no account configured for [{NotConfigured}]",
                projectId, accounts.Count, string.Join(", ", scan.Scanned),
                scan.Vms.Count, errors.Count, string.Join(", ", notConfigured));

            return Results.Ok(new InventoryResponse(
                scan.Vms, errors, scan.Scanned, notConfigured, DateTimeOffset.UtcNow));
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        return app;
    }

    /// <summary>The inventory wire response.</summary>
    internal sealed record InventoryResponse(
        [property: JsonPropertyName("vms")] IReadOnlyList<CloudVm> Vms,
        [property: JsonPropertyName("errors")] IReadOnlyList<string> Errors,
        [property: JsonPropertyName("scanned")] IReadOnlyList<string> Scanned,
        [property: JsonPropertyName("not_configured")] IReadOnlyList<string> NotConfigured,
        [property: JsonPropertyName("scanned_at")] DateTimeOffset ScannedAt);

    /// <summary>
    /// The project's cloud accounts, decrypted and ready to scan.
    ///
    /// <para>Returns three things: the accounts to enumerate, the
    /// <c>errors[]</c> lines for accounts that exist but cannot be scanned, and
    /// the set of providers the project has ANY account row for (so the caller
    /// can tell "not configured" — informational — from "configured but
    /// unusable" — an error).</para>
    ///
    /// <para>Only <c>active</c> accounts are scanned, matching every other
    /// consumer of <c>cloud_account</c> (provisioner, reaper, GCP key
    /// resolution). A pending/errored account is reported by name instead of
    /// being silently ignored.</para>
    /// </summary>
    internal static async Task<(List<InventoryAccount> Accounts, List<string> Errors, HashSet<string> Configured)>
        LoadAccountsAsync(
            NetworkerDbContext db,
            CredentialCipher? cipher,
            string projectId,
            ILogger logger,
            CancellationToken ct)
    {
        var accounts = new List<InventoryAccount>();
        var errors = new List<string>();
        var configured = new HashSet<string>(StringComparer.Ordinal);

        List<CloudAccountRow> rows;
        try
        {
            rows = await db.CloudAccounts
                .AsNoTracking()
                .Where(a => a.ProjectId == projectId)
                .OrderBy(a => a.CreatedAt)
                .Select(a => new CloudAccountRow(
                    a.Provider, a.Name, a.Status, a.RegionDefault, a.CredentialsEnc, a.CredentialsNonce))
                .ToListAsync(ct)
                .ConfigureAwait(false);
        }
        catch (Exception ex) when (ex is not OperationCanceledException)
        {
            logger.LogWarning(ex, "Inventory: reading project {ProjectId} cloud accounts failed", projectId);
            errors.Add("could not read this project's cloud accounts — nothing was scanned");
            return (accounts, errors, configured);
        }

        foreach (var row in rows)
        {
            configured.Add(row.Provider);

            if (!string.Equals(row.Status, "active", StringComparison.Ordinal))
            {
                errors.Add($"{row.Provider} account '{row.Name}': not scanned — the account is "
                           + $"'{row.Status}', not active (validate it under cloud accounts)");
                continue;
            }
            if (cipher is null)
            {
                // Bare host with no DASHBOARD_CREDENTIAL_KEY: say so once per
                // account rather than returning a silently empty list.
                errors.Add($"{row.Provider} account '{row.Name}': not scanned — this host has no "
                           + "credential key configured, so stored credentials cannot be decrypted");
                continue;
            }

            Dictionary<string, string> creds;
            try
            {
                creds = CredentialJson.ToMapLenient(
                    Encoding.UTF8.GetString(cipher.Decrypt(row.CredentialsEnc, row.CredentialsNonce)));
            }
            catch (Exception ex)
            {
                // Key rotation / corrupt nonce — same soft-fail posture as the
                // reaper and the GCP key resolver, but the user is told.
                logger.LogWarning("Inventory: cloud account {Account} failed to decrypt ({Error})", row.Name, ex.Message);
                errors.Add($"{row.Provider} account '{row.Name}': not scanned — the stored credentials "
                           + "failed to decrypt (re-enter them under cloud accounts)");
                continue;
            }

            accounts.Add(new InventoryAccount(row.Provider, row.Name, creds, row.RegionDefault));
        }

        return (accounts, errors, configured);
    }

    /// <summary>EF projection of the cloud_account columns the scan needs.</summary>
    internal sealed record CloudAccountRow(
        string Provider, string Name, string Status, string? RegionDefault,
        byte[] CredentialsEnc, byte[] CredentialsNonce);

    private static async Task<List<string>> GetManagedHostsAsync(NetworkerDbContext db, CancellationToken ct)
    {
        List<string?> endpointIpsJson;
        try
        {
            endpointIpsJson = await db.Deployments
                .AsNoTracking()
                .OrderByDescending(d => d.CreatedAt)
                .Take(100)
                .Select(d => d.EndpointIps)
                .ToListAsync(ct);
        }
        catch (Exception)
        {
            return new List<string>();
        }

        var hosts = new List<string>();
        foreach (var raw in endpointIpsJson)
        {
            if (string.IsNullOrEmpty(raw))
            {
                continue;
            }
            try
            {
                var arr = JsonSerializer.Deserialize<List<string>>(raw);
                if (arr is not null)
                {
                    hosts.AddRange(arr);
                }
            }
            catch (JsonException)
            {
                // Not a JSON string array — skip (matches Rust filter_map).
            }
        }
        return hosts;
    }

    /// <summary>
    /// True when the VM's fqdn or public_ip matches (exact or substring) any
    /// managed host. Mirrors the Rust <c>is_managed</c>; the implementation
    /// lives with the scanner that applies it.
    /// </summary>
    public static bool IsManaged(string? fqdn, string? publicIp, IReadOnlyList<string> managedHosts) =>
        CloudInventoryScanner.IsManaged(fqdn, publicIp, managedHosts);
}
