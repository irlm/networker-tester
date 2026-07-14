using System.Diagnostics;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// <see cref="IComputeProvisioner"/> that shells out to the cloud vendor CLIs —
/// the faithful C# port of the Rust dashboard's
/// <c>services::cloud_provider</c> Azure/AWS/GCP providers.
///
/// <para>
/// The command strings are kept 1:1 with the Rust source so behaviour is
/// identical to the shipping dashboard:
/// <list type="bullet">
///   <item><b>Azure</b> (<c>az</c>): <c>vm start</c> / <c>vm deallocate</c> /
///     <c>vm delete --yes</c> / <c>vm show --show-details</c>, each with explicit
///     <c>--subscription</c> + <c>--resource-group</c> + <c>--ids &lt;resource_id&gt;</c>
///     (never ambient defaults), <c>PYTHONWARNINGS=ignore</c> set.</item>
///   <item><b>AWS</b> (<c>aws ec2</c>): <c>start-instances</c> /
///     <c>stop-instances</c> / <c>terminate-instances</c> /
///     <c>describe-instances</c> with <c>--instance-ids &lt;resource_id&gt;</c>.</item>
///   <item><b>GCP</b> (<c>gcloud compute instances</c>): <c>start</c> / <c>stop</c>
///     / <c>delete --quiet</c> / <c>describe --format json</c> with the instance
///     name + <c>--zone</c> parsed out of the selfLink resource id.</item>
/// </list>
/// </para>
///
/// <para>
/// Process handling reuses the hardened pattern from
/// <c>Networker.Agent.ProbeRunner</c>: streams drained concurrently and awaited
/// after exit (no pipe-buffer deadlock), a hard timeout that kills the whole
/// process tree, and <c>UseShellExecute=false</c> + <c>CreateNoWindow=true</c>.
/// </para>
///
/// <para>
/// No method throws for infrastructure failure — a missing CLI or non-zero exit
/// becomes a <see cref="ProvisionResult"/> with <see cref="ProvisionResult.Success"/>
/// == false. Delete treats "resource already gone" as success (idempotent),
/// matching the Rust <c>ResourceNotFound</c> / <c>InvalidInstanceID.NotFound</c>
/// / <c>was not found</c> handling.
/// </para>
/// </summary>
public sealed class CliComputeProvisioner(ILogger<CliComputeProvisioner> logger) : IComputeProvisioner
{
    /// <summary>Hard ceiling for a single CLI invocation. Azure control-plane
    /// calls (deallocate especially) can take a while; the tree-kill guard
    /// prevents a wedged CLI from leaking.</summary>
    private static readonly TimeSpan CommandTimeout = TimeSpan.FromMinutes(5);

    public Task<ProvisionResult> StartAsync(
        ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct) =>
        DispatchAsync(tester, credentials, LifecycleOp.Start, ct);

    public Task<ProvisionResult> StopAsync(
        ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct) =>
        DispatchAsync(tester, credentials, LifecycleOp.Stop, ct);

    // Deallocate is the same wire op as Stop on every provider today (Azure's
    // stop == deallocate in the Rust source); kept as a distinct entry point.
    public Task<ProvisionResult> DeallocateAsync(
        ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct) =>
        DispatchAsync(tester, credentials, LifecycleOp.Stop, ct);

    public Task<ProvisionResult> DeleteAsync(
        ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct) =>
        DispatchAsync(tester, credentials, LifecycleOp.Delete, ct);

    public Task<ProvisionResult> ShowAsync(
        ProjectTester tester, ProviderCredentials? credentials, CancellationToken ct) =>
        DispatchAsync(tester, credentials, LifecycleOp.Show, ct);

    private enum LifecycleOp { Start, Stop, Delete, Show }

    private async Task<ProvisionResult> DispatchAsync(
        ProjectTester tester, ProviderCredentials? credentials, LifecycleOp op, CancellationToken ct)
    {
        var resourceId = tester.VmResourceId;
        if (string.IsNullOrEmpty(resourceId))
        {
            return ProvisionResult.SpawnError(
                "tester has no vm_resource_id; nothing to act on at the cloud layer");
        }

        var cloud = tester.Cloud?.ToLowerInvariant();
        var (file, args, env) = cloud switch
        {
            "azure" => BuildAzure(op, resourceId, credentials),
            "aws" => BuildAws(op, resourceId, credentials),
            "gcp" => BuildGcp(op, resourceId),
            _ => (null, null, null),
        };

        if (file is null || args is null)
        {
            return ProvisionResult.Unsupported(tester.Cloud ?? "(null)");
        }

        var result = await RunAsync(file, args, env, ct).ConfigureAwait(false);

        // Idempotent delete: "already gone" is the desired end-state. Mirrors the
        // Rust delete_vm paths that swallow ResourceNotFound / NotFound / 404.
        if (op == LifecycleOp.Delete && !result.Success && LooksAlreadyGone(result.StdErr))
        {
            logger.LogInformation(
                "{Cloud} resource {ResourceId} already gone; treating delete as success",
                cloud, resourceId);
            return ProvisionResult.Ok(result.ExitCode ?? 0, result.StdOut, result.StdErr);
        }

        return result;
    }

    // ── Azure argv (az) ──────────────────────────────────────────────────────

    private static (string file, List<string> args, IReadOnlyDictionary<string, string>? env)
        BuildAzure(LifecycleOp op, string resourceId, ProviderCredentials? creds)
    {
        // `az` resolution honours the AZ_CMD override the Rust az_bin() uses so
        // the same dev shim works across both stacks.
        var file = Environment.GetEnvironmentVariable("AZ_CMD") is { Length: > 0 } azOverride
            ? azOverride
            : "az";

        var sub = creds?.SubscriptionId ?? string.Empty;
        var rg = creds?.ResourceGroup ?? string.Empty;

        var verb = op switch
        {
            LifecycleOp.Start => "start",
            LifecycleOp.Stop => "deallocate", // Rust stop_vm → az vm deallocate
            LifecycleOp.Delete => "delete",
            _ => "show",
        };

        var args = new List<string> { "vm", verb, "--subscription", sub, "--resource-group", rg, "--ids", resourceId };
        if (op == LifecycleOp.Delete)
        {
            args.Add("--yes");
        }
        if (op == LifecycleOp.Show)
        {
            args.Add("--show-details");
            args.Add("--output");
            args.Add("json");
        }

        // PYTHONWARNINGS=ignore keeps az's Python SyntaxWarnings out of stdout so
        // JSON parsing stays clean — same as the Rust az_cmd().
        var env = new Dictionary<string, string> { ["PYTHONWARNINGS"] = "ignore" };
        return (file, args, env);
    }

    // ── AWS argv (aws ec2) ───────────────────────────────────────────────────

    private static (string file, List<string> args, IReadOnlyDictionary<string, string>? env)
        BuildAws(LifecycleOp op, string resourceId, ProviderCredentials? creds)
    {
        var verb = op switch
        {
            LifecycleOp.Start => "start-instances",
            LifecycleOp.Stop => "stop-instances",
            LifecycleOp.Delete => "terminate-instances",
            _ => "describe-instances",
        };

        var args = new List<string> { "ec2", verb, "--instance-ids", resourceId };
        if (op == LifecycleOp.Show)
        {
            args.Add("--query");
            args.Add("Reservations[0].Instances[0]");
            args.Add("--output");
            args.Add("json");
        }

        // Region + credentials via env, matching aws_cmd(). Access keys, if
        // present in Extra, are forwarded so a connection-scoped identity works;
        // otherwise the host's ambient profile is used.
        var env = new Dictionary<string, string>();
        if (creds?.Region is { Length: > 0 } region)
        {
            env["AWS_DEFAULT_REGION"] = region;
        }
        if (creds?.Extra is { } extra)
        {
            if (extra.TryGetValue("access_key_id", out var ak)) env["AWS_ACCESS_KEY_ID"] = ak;
            if (extra.TryGetValue("secret_access_key", out var sk)) env["AWS_SECRET_ACCESS_KEY"] = sk;
            if (extra.TryGetValue("session_token", out var st)) env["AWS_SESSION_TOKEN"] = st;
        }
        return (file: "aws", args, env: env.Count > 0 ? env : null);
    }

    // ── GCP argv (gcloud compute instances) ──────────────────────────────────

    private static (string file, List<string> args, IReadOnlyDictionary<string, string>? env)
        BuildGcp(LifecycleOp op, string resourceId)
    {
        var (name, zone) = ParseGcpResourceId(resourceId);

        var verb = op switch
        {
            LifecycleOp.Start => "start",
            LifecycleOp.Stop => "stop",
            LifecycleOp.Delete => "delete",
            _ => "describe",
        };

        var args = new List<string> { "compute", "instances", verb, name, "--zone", zone };
        if (op == LifecycleOp.Delete)
        {
            args.Add("--quiet");
        }
        if (op == LifecycleOp.Show)
        {
            args.Add("--format");
            args.Add("json");
        }
        return (file: "gcloud", args, env: null);
    }

    /// <summary>
    /// Parse a GCP selfLink resource id into (name, zone). Mirrors the Rust
    /// <c>parse_gcp_resource_id</c>. Format:
    /// <c>https://.../projects/PROJECT/zones/ZONE/instances/NAME</c>.
    /// </summary>
    internal static (string Name, string Zone) ParseGcpResourceId(string resourceId)
    {
        var parts = resourceId.Split('/');
        var name = parts.Length > 0 ? parts[^1] : string.Empty;
        var zone = string.Empty;
        for (var i = 0; i < parts.Length - 1; i++)
        {
            if (parts[i] == "zones")
            {
                zone = parts[i + 1];
                break;
            }
        }
        return (name, zone);
    }

    private static bool LooksAlreadyGone(string stderr) =>
        stderr.Contains("ResourceNotFound", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("could not be found", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("InvalidInstanceID.NotFound", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("does not exist", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("was not found", StringComparison.OrdinalIgnoreCase)
        || stderr.Contains("404", StringComparison.OrdinalIgnoreCase);

    // ── Hardened process runner (ported from ProbeRunner) ────────────────────

    private async Task<ProvisionResult> RunAsync(
        string file,
        List<string> args,
        IReadOnlyDictionary<string, string>? env,
        CancellationToken cancellationToken)
    {
        var psi = new ProcessStartInfo
        {
            FileName = file,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        foreach (var a in args)
        {
            psi.ArgumentList.Add(a);
        }
        if (env is not null)
        {
            foreach (var (k, v) in env)
            {
                psi.Environment[k] = v;
            }
        }

        logger.LogInformation("Provisioner spawning {File} {Args}", file, string.Join(' ', args));

        using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeoutCts.CancelAfter(CommandTimeout);
        var ct = timeoutCts.Token;

        using var process = new Process { StartInfo = psi };
        try
        {
            process.Start();
        }
        catch (Exception ex)
        {
            // The common CI path: the cloud CLI isn't installed. Soft failure —
            // the caller logs it and still returns 202 with the DB transition done.
            logger.LogWarning(ex, "Failed to launch provisioner CLI '{File}'", file);
            return ProvisionResult.SpawnError($"failed to launch '{file}': {ex.Message}");
        }

        // Drain both streams concurrently, await AFTER exit (avoids the
        // pipe-buffer deadlock + the BeginOutputReadLine flush race).
        var stdoutTask = process.StandardOutput.ReadToEndAsync(ct);
        var stderrTask = process.StandardError.ReadToEndAsync(ct);

        string stdout, stderr;
        try
        {
            await process.WaitForExitAsync(ct).ConfigureAwait(false);
            stdout = (await stdoutTask.ConfigureAwait(false)).Trim();
            stderr = (await stderrTask.ConfigureAwait(false)).Trim();
        }
        catch (OperationCanceledException) when (timeoutCts.IsCancellationRequested
                                                 && !cancellationToken.IsCancellationRequested)
        {
            KillTree(process);
            return ProvisionResult.SpawnError(
                $"provisioner CLI '{file}' timed out after {CommandTimeout.TotalSeconds:0}s and was killed");
        }
        catch (OperationCanceledException)
        {
            KillTree(process); // caller cancelled — don't leave the child running
            throw;
        }

        return process.ExitCode == 0
            ? ProvisionResult.Ok(process.ExitCode, stdout, stderr)
            : ProvisionResult.Failed(process.ExitCode, stdout, stderr);
    }

    private static void KillTree(Process process)
    {
        try
        {
            if (!process.HasExited)
            {
                process.Kill(entireProcessTree: true);
            }
        }
        catch
        {
            // Best-effort — may have exited between the check and the kill.
        }
    }
}
