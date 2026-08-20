using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Security;
using Networker.Data;
using Networker.Security;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// The GCP service-account credential an endpoint deploy's <c>install.sh</c>
/// needs in its environment (issue #833 — the #827 auth-bug class at its
/// fourth site).
///
/// <para>Endpoint deployments delegate VM creation to <c>install.sh --deploy</c>
/// on the control-plane host, and <see cref="DeployRunner"/> used to spawn it
/// with no cloud credentials at all — install.sh's GCP pre-flight then ran
/// <c>gcloud auth list</c>, found the host's (never interactively
/// authenticated) gcloud empty, and failed every GCP cell in seconds with
/// "Not authenticated to GCP". #827/#828 fixed the provisioner's own gcloud
/// calls, #830/#832 the zones; this threads the same account key into the
/// installer's process environment.</para>
///
/// <para>Mechanics mirror <see cref="CliComputeProvisioner"/>: the decrypted
/// <c>json_key</c> is written 0600 (long-lived credential, audit F11) inside a
/// 0700 throwaway directory that doubles as an isolated <c>CLOUDSDK_CONFIG</c>
/// — install.sh's <c>gcloud config set project</c> and any token cache land in
/// that directory, never in the host's shared gcloud session, and the whole
/// tree is deleted on <see cref="Dispose"/>. The env is
/// <see cref="CliComputeProvisioner.BuildGcloudEnv"/>'s
/// (<c>CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE</c> — the variable the gcloud CLI
/// actually reads — plus <c>GOOGLE_APPLICATION_CREDENTIALS</c> and
/// <c>CLOUDSDK_CORE_PROJECT</c>) plus <c>CLOUDSDK_CONFIG</c>.</para>
///
/// <para>Soft-fail posture, same as the orchestrator's zone resolution (#832):
/// any reason the key cannot be produced (no cipher on a bare test host, no
/// account, undecryptable credentials, no <c>json_key</c>) yields no env and a
/// human-readable note for the deployment log, and install.sh runs against the
/// host's ambient gcloud auth exactly as before.</para>
/// </summary>
internal sealed class GcpInstallerCredentials : IDisposable
{
    internal const string ConfigDirVar = "CLOUDSDK_CONFIG";

    /// <summary>What <see cref="PrepareAsync"/> decided. <see cref="Credentials"/>
    /// is null when nothing is threaded into the env; <see cref="Note"/> is the
    /// line for the deployment log (null only when the deploy has no GCP
    /// endpoint at all, where silence is the right output).</summary>
    internal sealed record Outcome(GcpInstallerCredentials? Credentials, string? Note)
    {
        public static readonly Outcome NotNeeded = new(null, null);
    }

    private readonly string _configDir;

    private GcpInstallerCredentials(
        string configDir, IReadOnlyDictionary<string, string> env, string? serviceAccount, string? projectId)
    {
        _configDir = configDir;
        Env = env;
        ServiceAccount = serviceAccount;
        ProjectId = projectId;
    }

    /// <summary>Environment variables to add to the install.sh process.</summary>
    public IReadOnlyDictionary<string, string> Env { get; }

    /// <summary>The key's <c>client_email</c> (for the log line), when present.</summary>
    public string? ServiceAccount { get; }

    /// <summary>The key's <c>project_id</c>, when present.</summary>
    public string? ProjectId { get; }

    /// <summary>True when any endpoint in <paramref name="deployJson"/> targets
    /// the <c>gcp</c> provider. Unparseable JSON is "no" — the runner's own
    /// deploy.json handling reports that, not this.</summary>
    internal static bool DeployNeedsGcp(string deployJson)
    {
        try
        {
            using var doc = JsonDocument.Parse(deployJson);
            if (!doc.RootElement.TryGetProperty("endpoints", out var endpoints)
                || endpoints.ValueKind != JsonValueKind.Array)
            {
                return false;
            }
            foreach (var ep in endpoints.EnumerateArray())
            {
                if (ep.ValueKind == JsonValueKind.Object
                    && ep.TryGetProperty("provider", out var provider)
                    && provider.ValueKind == JsonValueKind.String
                    && string.Equals(provider.GetString(), "gcp", StringComparison.OrdinalIgnoreCase))
                {
                    return true;
                }
            }
            return false;
        }
        catch (JsonException)
        {
            return false;
        }
    }

    /// <summary>
    /// Resolve the deployment's GCP account key and stage it for install.sh.
    /// The account is the deployment row's <c>cloud_account_id</c>
    /// (auto-provisioned comparison cells); wizard-created deployments carry no
    /// account id, so those fall back to the deployment's project having
    /// exactly one active GCP account. Never throws.
    /// </summary>
    public static async Task<Outcome> PrepareAsync(
        IServiceScopeFactory scopeFactory,
        Guid deploymentId,
        string deployJson,
        ILogger logger,
        CancellationToken ct)
    {
        if (!DeployNeedsGcp(deployJson))
        {
            return Outcome.NotNeeded;
        }

        try
        {
            return await PrepareCoreAsync(scopeFactory, deploymentId, logger, ct).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            throw;
        }
        catch (Exception ex)
        {
            logger.LogWarning(ex,
                "Deployment {DeploymentId}: staging the GCP account key for install.sh failed", deploymentId);
            return Skipped($"staging the account key failed: {ex.Message}");
        }
    }

    private static async Task<Outcome> PrepareCoreAsync(
        IServiceScopeFactory scopeFactory, Guid deploymentId, ILogger logger, CancellationToken ct)
    {
        using var scope = scopeFactory.CreateScope();
        var cipher = scope.ServiceProvider.GetService<CredentialCipher>();
        if (cipher is null)
        {
            return Skipped("no credential cipher is registered on this host");
        }

        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        var (jsonKey, why) = await ResolveGcpKeyForDeploymentAsync(db, cipher, deploymentId, logger, ct)
            .ConfigureAwait(false);
        if (jsonKey is null)
        {
            return Skipped(why ?? "no GCP service-account key");
        }

        var projectId = CliComputeProvisioner.ParseGcpProjectId(jsonKey);
        var serviceAccount = ParseClientEmail(jsonKey);

        // One 0700 directory holds the 0600 key AND serves as the isolated
        // gcloud config root, so a single recursive delete cleans up both.
        var configDir = Path.Combine(Path.GetTempPath(), $"gcp-deploy-{Guid.NewGuid():N}");
        SecretFile.CreateDir0700(configDir);
        var keyFile = Path.Combine(configDir, "key.json");
        try
        {
            await SecretFile.WriteAsync(keyFile, jsonKey, ct).ConfigureAwait(false);
        }
        catch
        {
            TryDeleteTree(configDir);
            throw;
        }

        var env = CliComputeProvisioner.BuildGcloudEnv(keyFile, projectId);
        env[ConfigDirVar] = configDir;

        var who = serviceAccount ?? "the deployment's GCP service-account key";
        var project = projectId is null ? "" : $", project {projectId}";
        return new Outcome(
            new GcpInstallerCredentials(configDir, env, serviceAccount, projectId),
            $"GCP credentials: {who}{project} handed to install.sh via CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE (isolated CLOUDSDK_CONFIG)");
    }

    /// <summary>
    /// The GCP service-account key a deployment acts under: its
    /// <c>cloud_account_id</c>'s account (auto-provisioned comparison cells),
    /// or — wizard deploys carry no account id — the deployment's project having
    /// exactly one active GCP account. Shared by the install.sh credential
    /// staging (#833) and the endpoint VM teardown (#838), so both resolve the
    /// same account. Returns <c>(null, reason)</c> when no key can be produced;
    /// never throws for data problems (an undecryptable account is a reason).
    /// </summary>
    internal static async Task<(string? JsonKey, string? Reason)> ResolveGcpKeyForDeploymentAsync(
        NetworkerDbContext db, CredentialCipher cipher, Guid deploymentId, ILogger logger, CancellationToken ct)
    {
        var row = await db.Deployments
            .AsNoTracking()
            .Where(d => d.DeploymentId == deploymentId)
            .Select(d => new { d.CloudAccountId, d.ProjectId })
            .FirstOrDefaultAsync(ct)
            .ConfigureAwait(false);
        if (row is null)
        {
            return (null, "deployment row not found");
        }

        var query = row.CloudAccountId is { } accountId
            ? db.CloudAccounts.AsNoTracking().Where(a => a.AccountId == accountId && a.Provider == "gcp")
            : db.CloudAccounts.AsNoTracking()
                .Where(a => a.ProjectId == row.ProjectId && a.Provider == "gcp" && a.Status == "active");
        var accounts = await query
            .OrderBy(a => a.CreatedAt)
            .Select(a => new { a.AccountId, a.Name, a.CredentialsEnc, a.CredentialsNonce })
            .Take(2)
            .ToListAsync(ct)
            .ConfigureAwait(false);

        if (accounts.Count == 0)
        {
            return (null, row.CloudAccountId is null
                ? "the project has no active GCP cloud account"
                : $"cloud account {row.CloudAccountId} is not a GCP account");
        }
        if (accounts.Count > 1)
        {
            // Wizard deploy in a project with several GCP accounts: there is no
            // signal which one the user meant — don't guess with someone's key.
            return (null, "the project has more than one active GCP cloud account and the deployment names none");
        }

        var acct = accounts[0];
        string? jsonKey;
        try
        {
            jsonKey = CredentialJson.ToMap(cipher.Decrypt(acct.CredentialsEnc, acct.CredentialsNonce))
                .GetValueOrDefault("json_key");
        }
        catch (Exception ex)
        {
            // Undecryptable account (key rotation, corrupt nonce) — same
            // soft-fail posture as the reaper and the zone resolver.
            logger.LogWarning(
                "Deployment {DeploymentId}: cloud account {Account} credentials failed to decrypt ({Error})",
                deploymentId, acct.Name, ex.Message);
            return (null, $"cloud account '{acct.Name}' credentials failed to decrypt");
        }
        if (string.IsNullOrEmpty(jsonKey))
        {
            return (null, $"cloud account '{acct.Name}' has no json_key");
        }
        return (jsonKey, null);
    }

    private static Outcome Skipped(string reason) => new(
        null,
        $"GCP credentials: {reason} — install.sh will rely on the host's ambient gcloud auth");

    /// <summary>Best-effort <c>client_email</c> from the key JSON (log line only).</summary>
    internal static string? ParseClientEmail(string jsonKey)
    {
        try
        {
            using var doc = JsonDocument.Parse(jsonKey);
            return doc.RootElement.TryGetProperty("client_email", out var email)
                   && email.ValueKind == JsonValueKind.String
                ? email.GetString()
                : null;
        }
        catch (JsonException)
        {
            return null;
        }
    }

    public void Dispose() => TryDeleteTree(_configDir);

    private static void TryDeleteTree(string dir)
    {
        try
        {
            Directory.Delete(dir, recursive: true);
        }
        catch
        {
            // best-effort temp cleanup; the dir is 0700 and holds nothing once
            // install.sh has exited, but a leftover is still preferable to a
            // crashed deploy worker.
        }
    }
}
