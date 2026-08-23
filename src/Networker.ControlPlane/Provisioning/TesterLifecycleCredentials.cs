using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.Data;
using Networker.Data.Entities;
using Networker.Security;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// The credentials every tester-LIFECYCLE cloud call runs under — auto-shutdown
/// deallocate, auto-wake start, the manual start/stop/force-stop/delete/probe
/// endpoints, and the agent auto-upgrade run-command. One implementation, so
/// the four sites that used to hold byte-identical private copies cannot drift
/// again.
///
/// <para><b>Azure/AWS (unchanged):</b> the tester's <c>cloud_connection</c>
/// row's <c>config</c> JSON, flattened into
/// <see cref="ProviderCredentials.Extra"/> with <c>subscription_id</c> /
/// <c>resource_group</c> / <c>region</c> lifted out. No connection (or an
/// unparseable config) still means <c>null</c> / bare credentials — the
/// provisioner then relies on the host's ambient CLI auth (managed identity,
/// instance profile), which is how prod Azure has always worked.</para>
///
/// <para><b>GCP (the #857 fix):</b> ambient auth is not a thing for gcloud. It
/// authenticates ONLY from its own config store or the per-invocation
/// <c>CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE</c> (#827), so on the prod host —
/// whose gcloud was never interactively authenticated — a credential-less
/// lifecycle call fails with "You do not currently have an active account
/// selected". The connection-only resolution never loaded the cloud ACCOUNT, so
/// every auto-shutdown tick for a GCP tester failed that way and an idle
/// <c>e2-standard-2</c> billed for three days. We now resolve and decrypt the
/// tester's service-account key through the SAME
/// <see cref="GcpInstallerCredentials"/> path the install.sh staging (#833),
/// the endpoint teardown (#838) and the inventory scan use, and hand it to the
/// provisioner in <c>Extra["json_key"]</c> — which
/// <see cref="CliComputeProvisioner"/> already knows how to turn into a 0600
/// key file plus <see cref="CliComputeProvisioner.BuildGcloudEnv"/>'s env.</para>
///
/// <para><b>Soft-fail posture:</b> a tester with no resolvable key (no cipher on
/// a bare host, no account, undecryptable credentials) degrades to exactly the
/// pre-#857 behaviour — ambient auth, one warning naming the reason. The reason
/// text never carries key material, and the key itself is never logged: it goes
/// straight into <see cref="ProviderCredentials.Extra"/> and from there into a
/// 0600 tempfile the provisioner deletes.</para>
/// </summary>
internal static class TesterLifecycleCredentials
{
    /// <summary>
    /// Resolve the credentials for one lifecycle call against
    /// <paramref name="tester"/>. Never throws for data problems.
    /// </summary>
    /// <param name="db">Scoped context; only read from.</param>
    /// <param name="cipher">The host's credential cipher, or null on a host that
    /// registers none (bare test/CI) — then GCP degrades to ambient auth.</param>
    internal static async Task<ProviderCredentials?> LoadAsync(
        NetworkerDbContext db,
        CredentialCipher? cipher,
        ProjectTester tester,
        ILogger logger,
        CancellationToken ct)
    {
        var creds = await FromConnectionAsync(db, tester, ct).ConfigureAwait(false);

        if (!string.Equals(tester.Cloud, "gcp", StringComparison.OrdinalIgnoreCase))
        {
            return creds;
        }

        // A GCP cloud_connection MAY already carry the key in its config; that
        // one is the operator's explicit choice for this tester, so it wins.
        if (creds?.Extra is { } present
            && present.TryGetValue("json_key", out var configured)
            && configured.Length > 0)
        {
            return creds;
        }

        var (jsonKey, why) = await GcpInstallerCredentials
            .ResolveGcpKeyForTesterAsync(db, cipher, tester, logger, ct)
            .ConfigureAwait(false);
        if (jsonKey is null)
        {
            logger.LogWarning(
                "Tester {TesterId} ({Name}): {Why} — the gcloud call will rely on the host's ambient auth "
                + "and will fail with \"no active account selected\" unless this host is authenticated",
                tester.TesterId, tester.Name, why ?? "no GCP service-account key");
            return creds;
        }

        var extra = creds?.Extra is { } existing
            ? new Dictionary<string, string>(existing, StringComparer.Ordinal)
            : new Dictionary<string, string>(StringComparer.Ordinal);
        extra["json_key"] = jsonKey;

        return creds is null
            ? new ProviderCredentials(tester.Cloud, Region: tester.Region, Extra: extra)
            : creds with { Extra = extra };
    }

    /// <summary>
    /// The historical connection-config resolution, verbatim: null when the
    /// tester has no <c>cloud_connection</c> (or the row is gone), and bare
    /// provider+region credentials when the config isn't readable JSON.
    /// </summary>
    private static async Task<ProviderCredentials?> FromConnectionAsync(
        NetworkerDbContext db, ProjectTester tester, CancellationToken ct)
    {
        if (tester.CloudConnectionId is not { } connId)
        {
            return null;
        }

        var conn = await db.CloudConnections.AsNoTracking()
            .FirstOrDefaultAsync(c => c.ConnectionId == connId, ct)
            .ConfigureAwait(false);
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
}
