using System.Diagnostics;
using System.Text;
using Microsoft.AspNetCore.Mvc;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Provisioning;
using Networker.ControlPlane.Security;
using Networker.Data;
using Networker.Data.Entities;
using Networker.Security;
using Npgsql;
using NpgsqlTypes;

namespace Networker.ControlPlane.Endpoints;

// Create + background cloud-init provisioning flow for TesterWriteEndpoints
// (route mapping + shared helpers live in TesterWriteEndpoints.cs).
public static partial class TesterWriteEndpoints
{
    // ── create ───────────────────────────────────────────────────────────────

    /// <summary>
    /// POST /testers — the C# port of the Rust <c>create_tester</c> handler
    /// (api/testers.rs): validate → rate-limit → connection/account gating →
    /// INSERT with the V027 COALESCE defaults → audit → spawn the cloud-init
    /// provisioning task → <c>202 Accepted</c> + the full tester row.
    /// </summary>
    private static async Task<IResult> CreateTester(
        string projectId,
        HttpContext http,
        [FromBody] CreateTesterBody? body,
        NetworkerDbContext db,
        NpgsqlDataSource dataSource,
        IServiceScopeFactory scopeFactory,
        ILoggerFactory loggerFactory,
        CancellationToken ct)
    {
        var user = http.GetAuthUser();
        var logger = loggerFactory.CreateLogger("TesterWrite.Create");

        // ?ssh_bootstrap=1 selected the Rust run_create_tester_ssh path (SSH-driven
        // installer instead of cloud-init). The frontend never sends it.
        // TODO(phase3): run_create_tester_ssh not ported — refuse explicitly
        // rather than silently falling back to the cloud-init path.
        if (http.Request.Query.TryGetValue("ssh_bootstrap", out var sshVals)
            && sshVals.Any(v => v is null or "" or "1" or "true"))
        {
            return ApiError.Status(
                StatusCodes.Status501NotImplemented,
                "ssh_bootstrap provisioning is not implemented in the C# control plane yet; "
                + "omit ssh_bootstrap to use the cloud-init path");
        }

        if (body is null)
        {
            return ApiError.BadRequest("Invalid request body");
        }

        if (TesterCreateLogic.ValidateCreateBody(body.Name, body.Cloud, body.Region) is { } invalid)
        {
            return ApiError.BadRequest(invalid);
        }

        // Rate-limit: total testers in project + creates in the last hour
        // (Rust: one query with a FILTER; two COUNTs are semantically identical).
        var hourAgo = DateTime.UtcNow.AddHours(-1);
        var total = await db.ProjectTesters.LongCountAsync(t => t.ProjectId == projectId, ct);
        var lastHour = await db.ProjectTesters.LongCountAsync(
            t => t.ProjectId == projectId && t.CreatedAt > hourAgo, ct);
        if (TesterCreateLogic.CheckRateLimit(total, lastHour) is { } limited)
        {
            return ApiError.Status(StatusCodes.Status429TooManyRequests, limited);
        }

        // Validate cloud_connection if provided: exists in project (404),
        // active (409), provider config parseable (400).
        if (body.CloudConnectionId is { } connId)
        {
            var conn = await db.CloudConnections.AsNoTracking()
                .FirstOrDefaultAsync(c => c.ConnectionId == connId && c.ProjectId == projectId, ct);
            if (conn is null)
            {
                return ApiError.NotFound($"cloud_connection {connId} not found in this project");
            }

            if (conn.Status != "active")
            {
                return Conflict($"cloud_connection {connId} status is '{conn.Status}', expected 'active'");
            }

            if (TesterCreateLogic.ValidateConnectionConfig(conn.Provider, conn.Config) is { } confErr)
            {
                return ApiError.BadRequest($"unsupported cloud provider: {confErr}");
            }
        }

        // Validate cloud_account if provided: exists in project (404), active
        // (409), provider matches the requested cloud (400).
        if (body.CloudAccountId is { } accountId)
        {
            var acct = await db.CloudAccounts.AsNoTracking()
                .FirstOrDefaultAsync(a => a.AccountId == accountId && a.ProjectId == projectId, ct);
            if (acct is null)
            {
                return ApiError.NotFound($"cloud_account {accountId} not found in this project");
            }

            if (acct.Status != "active")
            {
                return Conflict($"cloud_account {accountId} status is '{acct.Status}', expected 'active'");
            }

            if (acct.Provider != body.Cloud)
            {
                return ApiError.BadRequest(
                    $"cloud_account {accountId} is a '{acct.Provider}' account but the "
                    + $"tester cloud is '{body.Cloud}'");
            }
        }

        // Insert with the Rust db/project_testers.rs defaults (COALESCE resolved
        // up-front via ApplyDefaults — identical semantics, single source). Raw
        // SQL keeps the DB column defaults (power_state 'provisioning',
        // allocation 'idle', ssh_user 'azureuser', auto_shutdown TRUE) in charge,
        // exactly like the Rust INSERT.
        var defaults = TesterCreateLogic.ApplyDefaults(
            body.VmSize, body.AutoShutdownLocalHour, body.AutoProbeEnabled,
            body.RequestedOs, body.RequestedVariant);

        Guid testerId;
        await using (var conn = await dataSource.OpenConnectionAsync(ct))
        await using (var cmd = conn.CreateCommand())
        {
            cmd.CommandText = """
                INSERT INTO project_tester (
                    project_id, name, cloud, region,
                    vm_size, auto_shutdown_local_hour, auto_probe_enabled,
                    created_by, cloud_connection_id, cloud_account_id,
                    requested_os, requested_variant
                ) VALUES (
                    @project_id, @name, @cloud, @region,
                    @vm_size, @hour, @probe,
                    @created_by, @conn_id, @account_id,
                    @os, @variant
                )
                RETURNING tester_id
                """;
            cmd.Parameters.AddWithValue("project_id", projectId);
            cmd.Parameters.AddWithValue("name", body.Name);
            cmd.Parameters.AddWithValue("cloud", body.Cloud);
            cmd.Parameters.AddWithValue("region", body.Region);
            cmd.Parameters.AddWithValue("vm_size", defaults.VmSize);
            cmd.Parameters.AddWithValue("hour", defaults.AutoShutdownLocalHour);
            cmd.Parameters.AddWithValue("probe", defaults.AutoProbeEnabled);
            cmd.Parameters.AddWithValue("created_by", user?.UserId ?? Guid.Empty);
            cmd.Parameters.Add(new NpgsqlParameter("conn_id", NpgsqlDbType.Uuid)
            {
                Value = (object?)body.CloudConnectionId ?? DBNull.Value,
            });
            cmd.Parameters.Add(new NpgsqlParameter("account_id", NpgsqlDbType.Uuid)
            {
                Value = (object?)body.CloudAccountId ?? DBNull.Value,
            });
            cmd.Parameters.AddWithValue("os", defaults.RequestedOs);
            cmd.Parameters.AddWithValue("variant", defaults.RequestedVariant);

            try
            {
                testerId = (Guid)(await cmd.ExecuteScalarAsync(ct))!;
            }
            catch (PostgresException pg) when (pg.SqlState == PostgresErrorCodes.UniqueViolation)
            {
                // UNIQUE (project_id, name) — surface as 409 rather than a 500.
                return Conflict($"a tester named '{body.Name}' already exists in this project");
            }
        }

        var row = await db.ProjectTesters.AsNoTracking().FirstAsync(t => t.TesterId == testerId, ct);

        logger.LogInformation(
            "tester {TesterId} created by {Actor} in project {ProjectId} "
            + "region={Region} vm_size={VmSize} (provisioning in background)",
            testerId, user?.Email, projectId, row.Region, row.VmSize);

        // Rust audit_tester_action: structured log sink only (no service_log
        // table) — action=tester_created outcome=requested.
        logger.LogInformation(
            "tester action audited: project_id={ProjectId} tester_id={TesterId} actor_user_id={ActorUserId} "
            + "action={Action} outcome={Outcome} message={Message}",
            projectId, testerId, user?.UserId, "tester_created", "requested",
            $"region={row.Region} vm_size={row.VmSize}");

        FireAndForgetCreate(scopeFactory, loggerFactory, projectId, testerId);

        return Results.Accepted($"/api/projects/{projectId}/testers/{testerId}", ToFullDto(row));
    }

    /// <summary>
    /// Bounded create concurrency — stands in for the Rust
    /// <c>state.deploy_semaphore</c> (DEPLOY_CONCURRENCY env, default 2).
    /// Divergence note: process-wide static instead of app-state, but the
    /// effect (at most N concurrent cloud creates per process) is identical.
    /// </summary>
    private static readonly SemaphoreSlim CreateSemaphore = BuildCreateSemaphore();

    private static SemaphoreSlim BuildCreateSemaphore()
    {
        var n = int.TryParse(Environment.GetEnvironmentVariable("DEPLOY_CONCURRENCY"), out var v) && v > 0
            ? v
            : 2;
        return new SemaphoreSlim(n, n);
    }

    /// <summary>
    /// Rust <c>spawn_create_tester_task</c>: run the cloud-init provisioning
    /// flow detached; on any failure stamp <c>power_state='error'</c> +
    /// <c>status_message='create failed: …'</c>.
    /// </summary>
    private static void FireAndForgetCreate(
        IServiceScopeFactory scopeFactory,
        ILoggerFactory loggerFactory,
        string projectId,
        Guid testerId)
    {
        var logger = loggerFactory.CreateLogger("TesterWrite.create.bg");
        _ = Task.Run(async () =>
        {
            await CreateSemaphore.WaitAsync().ConfigureAwait(false);
            try
            {
                using var scope = scopeFactory.CreateScope();
                await RunCreateTesterCloudInitAsync(scope.ServiceProvider, projectId, testerId, logger)
                    .ConfigureAwait(false);
            }
            catch (Exception ex)
            {
                logger.LogError(ex, "tester create background task failed for {TesterId}", testerId);
                try
                {
                    using var scope = scopeFactory.CreateScope();
                    var ds = scope.ServiceProvider.GetRequiredService<NpgsqlDataSource>();
                    await using var conn = await ds.OpenConnectionAsync().ConfigureAwait(false);
                    await using var cmd = conn.CreateCommand();
                    cmd.CommandText = "UPDATE project_tester SET power_state='error', "
                                      + "status_message=@msg, updated_at=NOW() WHERE tester_id=@tester";
                    cmd.Parameters.AddWithValue("msg", $"create failed: {ex.Message}");
                    cmd.Parameters.AddWithValue("tester", testerId);
                    await cmd.ExecuteNonQueryAsync().ConfigureAwait(false);
                }
                catch (Exception persistEx)
                {
                    logger.LogError(persistEx, "failed to record create failure for {TesterId}", testerId);
                }
            }
            finally
            {
                CreateSemaphore.Release();
            }
        });
    }

    /// <summary>
    /// The cloud-init provisioning flow — the C# port of the Rust
    /// <c>run_create_tester_cloud_init</c>: mint the agent api-key BEFORE VM
    /// create, bake it into the bootstrap script, create the VM, persist the
    /// identity fields, then poll for the agent registering itself online.
    /// No SSH is performed from the control plane.
    /// </summary>
    private static async Task RunCreateTesterCloudInitAsync(
        IServiceProvider sp, string projectId, Guid testerId, ILogger logger)
    {
        var db = sp.GetRequiredService<NetworkerDbContext>();
        var dataSource = sp.GetRequiredService<NpgsqlDataSource>();
        var cipher = sp.GetRequiredService<CredentialCipher>();
        var provisioner = sp.GetRequiredService<IComputeProvisioner>();
        var recorder = sp.GetRequiredService<IVmLifecycleRecorder>();

        var tester = await db.ProjectTesters.AsNoTracking()
                         .FirstOrDefaultAsync(t => t.TesterId == testerId).ConfigureAwait(false)
                     ?? throw new InvalidOperationException("tester row disappeared before provisioning started");
        var region = tester.Region;
        var vmSize = tester.VmSize;

        await using var conn = await dataSource.OpenConnectionAsync().ConfigureAwait(false);
        await TesterState.SetStatusMessageAsync(conn, testerId, "minting agent key").ConfigureAwait(false);

        var vmNamePreview = TesterCreateLogic.GenerateVmName(region);

        // Step 1: mint the agent api_key before VM create so it can be baked
        // into the bootstrap (Rust provision_agent_for_tester). Only the hash is
        // persisted — auth looks up api_key_hash (V040) and the plaintext column
        // was dropped in V045. The plaintext key lives only in memory here, just
        // long enough to bake into the bootstrap.
        var agentApiKey = TesterCreateLogic.GenerateAgentApiKey();
        await using (var cmd = conn.CreateCommand())
        {
            cmd.CommandText = """
                INSERT INTO agent (agent_id, name, api_key_hash, region, provider, project_id, tester_id)
                VALUES (@agent_id, @name, @api_key_hash, @region, @provider, @project_id, @tester_id)
                """;
            cmd.Parameters.AddWithValue("agent_id", Guid.NewGuid());
            cmd.Parameters.AddWithValue("name", vmNamePreview);
            cmd.Parameters.AddWithValue("api_key_hash", AgentApiKeys.HashHex(agentApiKey));
            cmd.Parameters.AddWithValue("region", region);
            cmd.Parameters.AddWithValue("provider", tester.Cloud);
            cmd.Parameters.AddWithValue("project_id", tester.ProjectId);
            cmd.Parameters.AddWithValue("tester_id", testerId);
            await cmd.ExecuteNonQueryAsync().ConfigureAwait(false);
        }

        logger.LogInformation("Linked new agent row to persistent tester {TesterId}", testerId);

        // Step 2: resolve provider credentials + image + bootstrap script.
        var creds = await ResolveProviderCredentialsAsync(db, cipher, tester).ConfigureAwait(false);
        var requestedOs = tester.RequestedOs ?? "ubuntu-24.04";
        var requestedVariant = tester.RequestedVariant ?? "server";
        var image = TesterCreateLogic.ResolveImage(tester.Cloud, requestedOs, requestedVariant);
        var sshUser = TesterCreateLogic.DefaultSshUser(tester.Cloud, requestedOs);
        var targetTriple = TesterCreateLogic.TargetTripleFor(requestedOs);
        var isWindows = requestedOs.StartsWith("windows", StringComparison.Ordinal);

        var agentWs = CloudInitScripts.AgentWsUrl(CollabConfig.PublicUrl());
        if (agentWs.Contains("localhost", StringComparison.Ordinal)
            || agentWs.Contains("127.0.0.1", StringComparison.Ordinal))
        {
            logger.LogWarning(
                "DASHBOARD_PUBLIC_URL resolves to localhost ({AgentWs}) — the cloud VM's agent will NOT "
                + "be able to reach this dashboard. Set DASHBOARD_PUBLIC_URL to a publicly reachable "
                + "URL before provisioning cloud runners.", agentWs);
        }

        string bootstrap;
        if (isWindows)
        {
            var raw = CloudInitScripts.RenderWindowsBootstrap(agentWs, agentApiKey, targetTriple);
            // AWS user-data convention: wrap PowerShell in <powershell>...</powershell>.
            bootstrap = tester.Cloud.Equals("aws", StringComparison.OrdinalIgnoreCase)
                ? $"<powershell>\n{raw}\n</powershell>"
                : raw;
        }
        else
        {
            bootstrap = CloudInitScripts.RenderLinuxBootstrap(agentWs, agentApiKey, targetTriple);
        }

        logger.LogInformation(
            "Resolved OS image + bootstrap script (cloud-init path): cloud={Cloud} os={Os} variant={Variant} "
            + "image={Image} ssh_user={SshUser} target_triple={Triple} bootstrap_bytes={Bytes}",
            tester.Cloud, requestedOs, requestedVariant, image, sshUser, targetTriple, bootstrap.Length);

        // TODO(phase3): the Rust path runs a pre-create cloud orphan reaper here
        // (cloud_orphan_reaper::list_orphans/delete_orphans, soft-fail, 30s cap)
        // to avoid Azure public-IP quota exhaustion from prior failed creates.
        // Not ported yet — the Rust flow soft-fails it, so skipping loses only
        // the pre-emptive quota cleanup, never correctness.

        var createPhaseMsg = isWindows
            ? "creating VM + running Windows bootstrap via CustomScriptExtension (5-10 min)"
            : "creating VM + running cloud-init bootstrap (~60-120s)";
        await TesterState.SetStatusMessageAsync(conn, testerId, createPhaseMsg).ConfigureAwait(false);

        var created = await provisioner.CreateVmAsync(
            new VmCreateRequest(tester.Cloud, vmNamePreview, region, vmSize, sshUser, image, bootstrap),
            creds).ConfigureAwait(false);
        if (!created.Success)
        {
            // A failed create can still carry a KNOWN resource id — e.g. the VM
            // was created but a follow-up step (Windows extension set) failed
            // (quality audit F8). Persist that resource id onto the tester row and
            // log it loudly so the orphan reaper / an operator can clean up the
            // billing VM instead of losing it entirely.
            if (!string.IsNullOrEmpty(created.ResourceId))
            {
                logger.LogError(
                    "VM create for tester {TesterId} failed AFTER the VM was created "
                    + "(resource_id={ResourceId}): {Error} — orphaned VM, manual/reaper "
                    + "cleanup required; persisting vm_resource_id so it can be reclaimed.",
                    testerId, created.ResourceId, created.Error ?? "VM create failed");

                await using var orphanCmd = conn.CreateCommand();
                orphanCmd.CommandText = """
                    UPDATE project_tester
                       SET vm_name = @vm_name, vm_resource_id = @resource_id, updated_at = NOW()
                     WHERE tester_id = @tester
                    """;
                orphanCmd.Parameters.AddWithValue("vm_name", created.VmName ?? vmNamePreview);
                orphanCmd.Parameters.AddWithValue("resource_id", created.ResourceId);
                orphanCmd.Parameters.AddWithValue("tester", testerId);
                await orphanCmd.ExecuteNonQueryAsync().ConfigureAwait(false);
            }

            throw new InvalidOperationException(created.Error ?? "VM create failed");
        }

        // Step 3: persist identity fields (Rust: two UPDATE shapes — with and
        // without a public IP).
        if (string.IsNullOrEmpty(created.PublicIp))
        {
            await using var cmd = conn.CreateCommand();
            cmd.CommandText = """
                UPDATE project_tester
                   SET vm_name = @vm_name, vm_resource_id = @resource_id,
                       ssh_user = @ssh_user, updated_at = NOW()
                 WHERE tester_id = @tester
                """;
            cmd.Parameters.AddWithValue("vm_name", created.VmName ?? vmNamePreview);
            cmd.Parameters.AddWithValue("resource_id", created.ResourceId ?? string.Empty);
            cmd.Parameters.AddWithValue("ssh_user", sshUser);
            cmd.Parameters.AddWithValue("tester", testerId);
            await cmd.ExecuteNonQueryAsync().ConfigureAwait(false);
        }
        else
        {
            if (!System.Net.IPAddress.TryParse(created.PublicIp, out var ip))
            {
                throw new InvalidOperationException($"invalid public_ip '{created.PublicIp}'");
            }

            await using var cmd = conn.CreateCommand();
            cmd.CommandText = """
                UPDATE project_tester
                   SET vm_name = @vm_name, vm_resource_id = @resource_id, public_ip = @public_ip,
                       ssh_user = @ssh_user, updated_at = NOW()
                 WHERE tester_id = @tester
                """;
            cmd.Parameters.AddWithValue("vm_name", created.VmName ?? vmNamePreview);
            cmd.Parameters.AddWithValue("resource_id", created.ResourceId ?? string.Empty);
            cmd.Parameters.Add(new NpgsqlParameter("public_ip", NpgsqlDbType.Inet) { Value = ip });
            cmd.Parameters.AddWithValue("ssh_user", sshUser);
            cmd.Parameters.AddWithValue("tester", testerId);
            await cmd.ExecuteNonQueryAsync().ConfigureAwait(false);
        }

        var waitHint = isWindows
            ? "waiting for agent to come online (Windows: 2-5 min after VM boot)"
            : "waiting for agent to come online (Linux: usually < 30s)";
        await TesterState.SetStatusMessageAsync(conn, testerId, waitHint).ConfigureAwait(false);

        // Lifecycle events: the VM was created + booted — emit BOTH `created`
        // and `started` (Rust record_tester_lifecycle × 2), snapshotting the
        // persisted vm_name / vm_resource_id the UPDATE above just wrote.
        var fresh = await db.ProjectTesters.AsNoTracking()
            .FirstOrDefaultAsync(t => t.ProjectId == projectId && t.TesterId == testerId)
            .ConfigureAwait(false);
        if (fresh is not null)
        {
            var eventTime = DateTime.UtcNow;
            foreach (var eventType in new[] { "created", "started" })
            {
                await recorder.RecordTesterEventAsync(new TesterEventInput(
                    fresh.ProjectId, fresh.TesterId, fresh.Name, fresh.Cloud, fresh.Region,
                    fresh.VmSize, fresh.VmName, fresh.VmResourceId, fresh.CloudConnectionId,
                    eventType, eventTime, fresh.CreatedBy, null)).ConfigureAwait(false);
            }
        }

        // Step 4: poll for the agent reporting online. Windows takes much
        // longer (choco + npcap + wireshark first); Linux is capped at 10 min
        // to absorb slow apt mirrors + GitHub API rate-limit retries — same
        // budgets as the Rust source.
        var timeoutSecs = isWindows ? 900 : 600;
        var stopwatch = Stopwatch.StartNew();
        var observedOnline = false;
        const int statusUpdateEvery = 6; // 6 ticks × 5s = 30s
        var tick = 0;
        while (true)
        {
            var status = await db.Agents.AsNoTracking()
                .Where(a => a.TesterId == testerId)
                .Select(a => a.Status)
                .FirstOrDefaultAsync().ConfigureAwait(false);
            if (status == "online")
            {
                observedOnline = true;
                break;
            }

            if (stopwatch.Elapsed.TotalSeconds >= timeoutSecs)
            {
                break;
            }

            // Periodic elapsed-time refresh so the polling Testers page can
            // tell the control plane is still working, not stalled.
            tick++;
            if (tick % statusUpdateEvery == 0)
            {
                var elapsed = (long)stopwatch.Elapsed.TotalSeconds;
                var remaining = Math.Max(0, timeoutSecs - elapsed);
                try
                {
                    await TesterState.SetStatusMessageAsync(
                        conn, testerId,
                        $"waiting for agent ({elapsed}s elapsed, up to {remaining}s remaining)")
                        .ConfigureAwait(false);
                }
                catch (Exception ex)
                {
                    // Best-effort like the Rust `let _ =` — a transient DB hiccup
                    // must not abort the wait.
                    logger.LogDebug(ex, "status refresh failed for {TesterId}", testerId);
                }
            }

            await Task.Delay(TimeSpan.FromSeconds(5)).ConfigureAwait(false);
        }

        if (!observedOnline)
        {
            var msg = $"agent did not come online within {timeoutSecs}s";
            await using (var cmd = conn.CreateCommand())
            {
                cmd.CommandText = "UPDATE project_tester SET power_state='error', "
                                  + "status_message=@msg, updated_at=NOW() WHERE tester_id=@tester";
                cmd.Parameters.AddWithValue("msg", msg);
                cmd.Parameters.AddWithValue("tester", testerId);
                await cmd.ExecuteNonQueryAsync().ConfigureAwait(false);
            }

            throw new InvalidOperationException(msg);
        }

        // Step 5: provisioning → running + stamp installer_version + shutdown.
        // OS-info columns stay NULL on this path (only the SSH-driven flows
        // populate them), matching Rust.
        var moved = await TesterState.TryPowerTransitionAsync(conn, testerId, "provisioning", "running")
            .ConfigureAwait(false);
        if (!moved)
        {
            logger.LogWarning(
                "power_state was not 'provisioning' at end of cloud-init wait for {TesterId}", testerId);
        }

        await using (var cmd = conn.CreateCommand())
        {
            cmd.CommandText = """
                UPDATE project_tester
                   SET installer_version = @version, last_installed_at = NOW(),
                       status_message = NULL, updated_at = NOW()
                 WHERE tester_id = @tester
                """;
            cmd.Parameters.AddWithValue("version", VersionEndpoints.DashboardVersion);
            cmd.Parameters.AddWithValue("tester", testerId);
            await cmd.ExecuteNonQueryAsync().ConfigureAwait(false);
        }

        await using (var cmd = conn.CreateCommand())
        {
            cmd.CommandText = "UPDATE project_tester SET next_shutdown_at = NOW() + INTERVAL '15 hours' "
                              + "WHERE tester_id = @tester AND auto_shutdown_enabled = TRUE";
            cmd.Parameters.AddWithValue("tester", testerId);
            await cmd.ExecuteNonQueryAsync().ConfigureAwait(false);
        }

        logger.LogInformation("tester {TesterId} provisioning complete (cloud-init)", testerId);
    }

    /// <summary>
    /// Resolve the cloud credentials/config used for the create call — the C#
    /// port of the Rust <c>provider_for_tester</c>: (1) cloud_connection
    /// (secretless config), (2) cloud_account (encrypted credentials — the
    /// explicitly bound account first, else the oldest active account for the
    /// provider), (3) legacy AZURE_SUBSCRIPTION_ID env fallback. Throws with
    /// the Rust error messages on failure.
    /// </summary>
    internal static async Task<ProviderCredentials> ResolveProviderCredentialsAsync(
        NetworkerDbContext db, CredentialCipher cipher, ProjectTester tester)
    {
        // 1. Cloud connection (FIC/secretless).
        if (tester.CloudConnectionId is { } connId)
        {
            var conn = await db.CloudConnections.AsNoTracking()
                           .FirstOrDefaultAsync(c => c.ConnectionId == connId).ConfigureAwait(false)
                       ?? throw new InvalidOperationException($"cloud_connection {connId} not found");
            if (TesterCreateLogic.ValidateConnectionConfig(conn.Provider, conn.Config) is { } err)
            {
                throw new InvalidOperationException(err);
            }

            var extra = CredentialJson.ToMapLenient(conn.Config);
            extra.TryGetValue("subscription_id", out var sub);
            extra.TryGetValue("resource_group", out var rg);
            var region = extra.TryGetValue("region", out var r) && r.Length > 0 ? r : tester.Region;
            return new ProviderCredentials(conn.Provider, sub, rg, region, extra);
        }

        // 2. Cloud account (encrypted credentials). Prefer the account chosen at
        //    creation time; fall back to the oldest active account for provider.
        CloudAccount? acct;
        if (tester.CloudAccountId is { } accountId)
        {
            acct = await db.CloudAccounts.AsNoTracking()
                .FirstOrDefaultAsync(a => a.AccountId == accountId
                                          && a.ProjectId == tester.ProjectId
                                          && a.Status == "active").ConfigureAwait(false);
            if (acct is null)
            {
                throw new InvalidOperationException(
                    $"cloud account {accountId} selected for this tester is no longer active or was "
                    + "removed — edit the tester or re-create it with a valid account");
            }
        }
        else
        {
            acct = await db.CloudAccounts.AsNoTracking()
                .Where(a => a.ProjectId == tester.ProjectId
                            && a.Provider == tester.Cloud
                            && a.Status == "active")
                .OrderBy(a => a.CreatedAt)
                .FirstOrDefaultAsync().ConfigureAwait(false);
        }

        if (acct is not null)
        {
            var plaintext = cipher.Decrypt(acct.CredentialsEnc, acct.CredentialsNonce);
            var creds = CredentialJson.ToMapLenient(Encoding.UTF8.GetString(plaintext));

            switch (tester.Cloud)
            {
                case "azure":
                {
                    var rg = creds.TryGetValue("resource_group", out var g) && g.Length > 0
                        ? g
                        : "networker-testers";
                    var extra = new Dictionary<string, string>(StringComparer.Ordinal)
                    {
                        ["subscription_id"] = creds.GetValueOrDefault("subscription_id", string.Empty),
                        ["resource_group"] = rg,
                        ["tenant_id"] = creds.GetValueOrDefault("tenant_id", string.Empty),
                        ["client_id"] = creds.GetValueOrDefault("client_id", string.Empty),
                        ["client_secret"] = creds.GetValueOrDefault("client_secret", string.Empty),
                        ["identity_type"] = "service_principal",
                    };
                    return new ProviderCredentials(
                        "azure", extra["subscription_id"], rg, tester.Region, extra);
                }

                case "aws":
                {
                    if (string.IsNullOrEmpty(creds.GetValueOrDefault("access_key_id"))
                        || string.IsNullOrEmpty(creds.GetValueOrDefault("secret_access_key")))
                    {
                        throw new InvalidOperationException(
                            "aws config: missing access_key_id or secret_access_key");
                    }

                    var extra = new Dictionary<string, string>(creds, StringComparer.Ordinal)
                    {
                        ["region"] = tester.Region,
                    };
                    return new ProviderCredentials("aws", null, null, tester.Region, extra);
                }

                case "gcp":
                {
                    if (string.IsNullOrEmpty(creds.GetValueOrDefault("json_key")))
                    {
                        throw new InvalidOperationException("gcp config: missing json_key");
                    }

                    var extra = new Dictionary<string, string>(creds, StringComparer.Ordinal)
                    {
                        ["region"] = tester.Region,
                    };
                    return new ProviderCredentials("gcp", null, null, tester.Region, extra);
                }

                default:
                    return new ProviderCredentials(tester.Cloud, null, null, tester.Region, creds);
            }
        }

        // 3. Legacy env-var fallback (Rust legacy_azure_provider).
        var subEnv = Environment.GetEnvironmentVariable("AZURE_SUBSCRIPTION_ID");
        if (string.IsNullOrEmpty(subEnv))
        {
            subEnv = Environment.GetEnvironmentVariable("DASHBOARD_AZURE_SUBSCRIPTION");
        }

        if (string.IsNullOrEmpty(subEnv))
        {
            throw new InvalidOperationException(
                "No Azure subscription configured. Either:\n"
                + "1. Add a Cloud Account (Settings > Cloud > Add Account) with Azure credentials, or\n"
                + "2. Add a Cloud Connection (Settings > Cloud Connections) with managed identity config, or\n"
                + "3. Set AZURE_SUBSCRIPTION_ID environment variable on the dashboard");
        }

        var rgEnv = Environment.GetEnvironmentVariable("DASHBOARD_AZURE_RG");
        if (string.IsNullOrEmpty(rgEnv))
        {
            rgEnv = "networker-testers";
        }

        return new ProviderCredentials(
            "azure", subEnv, rgEnv, tester.Region,
            new Dictionary<string, string>(StringComparer.Ordinal)
            {
                ["identity_type"] = "managed_identity",
            });
    }

}
