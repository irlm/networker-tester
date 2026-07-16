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

/// <summary>
/// Persistent tester (VM) <b>lifecycle write</b> endpoints — the C# port of the
/// mutating handlers in the Rust dashboard's <c>api/testers.rs</c>
/// (create / start / stop / upgrade / probe / postpone / force-stop / schedule / delete).
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

        // POST /testers — create + provision (Operator). The collection route;
        // the read-side GET lives in TestersEndpoints.
        app.MapPost("/api/projects/{projectId}/testers", CreateTester)
            .RequireAuthorization(AuthPolicies.ProjectOperator);

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
            return Results.Json(
                new
                {
                    error = "ssh_bootstrap provisioning is not implemented in the C# control plane yet; "
                            + "omit ssh_bootstrap to use the cloud-init path",
                },
                statusCode: StatusCodes.Status501NotImplemented);
        }

        if (body is null)
        {
            return Results.BadRequest(new { error = "Invalid request body" });
        }

        if (TesterCreateLogic.ValidateCreateBody(body.Name, body.Cloud, body.Region) is { } invalid)
        {
            return Results.BadRequest(new { error = invalid });
        }

        // Rate-limit: total testers in project + creates in the last hour
        // (Rust: one query with a FILTER; two COUNTs are semantically identical).
        var hourAgo = DateTime.UtcNow.AddHours(-1);
        var total = await db.ProjectTesters.LongCountAsync(t => t.ProjectId == projectId, ct);
        var lastHour = await db.ProjectTesters.LongCountAsync(
            t => t.ProjectId == projectId && t.CreatedAt > hourAgo, ct);
        if (TesterCreateLogic.CheckRateLimit(total, lastHour) is { } limited)
        {
            return Results.Json(new { error = limited }, statusCode: StatusCodes.Status429TooManyRequests);
        }

        // Validate cloud_connection if provided: exists in project (404),
        // active (409), provider config parseable (400).
        if (body.CloudConnectionId is { } connId)
        {
            var conn = await db.CloudConnections.AsNoTracking()
                .FirstOrDefaultAsync(c => c.ConnectionId == connId && c.ProjectId == projectId, ct);
            if (conn is null)
            {
                return Results.NotFound(new { error = $"cloud_connection {connId} not found in this project" });
            }

            if (conn.Status != "active")
            {
                return Conflict($"cloud_connection {connId} status is '{conn.Status}', expected 'active'");
            }

            if (TesterCreateLogic.ValidateConnectionConfig(conn.Provider, conn.Config) is { } confErr)
            {
                return Results.BadRequest(new { error = $"unsupported cloud provider: {confErr}" });
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
                return Results.NotFound(new { error = $"cloud_account {accountId} not found in this project" });
            }

            if (acct.Status != "active")
            {
                return Conflict($"cloud_account {accountId} status is '{acct.Status}', expected 'active'");
            }

            if (acct.Provider != body.Cloud)
            {
                return Results.BadRequest(new
                {
                    error = $"cloud_account {accountId} is a '{acct.Provider}' account but the "
                            + $"tester cloud is '{body.Cloud}'",
                });
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
        // into the bootstrap (Rust provision_agent_for_tester).
        var agentApiKey = TesterCreateLogic.GenerateAgentApiKey();
        await using (var cmd = conn.CreateCommand())
        {
            cmd.CommandText = """
                INSERT INTO agent (agent_id, name, api_key, region, provider, project_id, tester_id)
                VALUES (@agent_id, @name, @api_key, @region, @provider, @project_id, @tester_id)
                """;
            cmd.Parameters.AddWithValue("agent_id", Guid.NewGuid());
            cmd.Parameters.AddWithValue("name", vmNamePreview);
            cmd.Parameters.AddWithValue("api_key", agentApiKey);
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

            var extra = FlattenJsonObject(conn.Config);
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
            var creds = FlattenJsonObject(Encoding.UTF8.GetString(plaintext));

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

    /// <summary>Flatten a JSON object into a string→string map (string values
    /// as-is, non-strings as raw JSON). Non-object / invalid JSON → empty.</summary>
    private static Dictionary<string, string> FlattenJsonObject(string json)
    {
        var map = new Dictionary<string, string>(StringComparer.Ordinal);
        try
        {
            using var doc = JsonDocument.Parse(json);
            if (doc.RootElement.ValueKind == JsonValueKind.Object)
            {
                foreach (var prop in doc.RootElement.EnumerateObject())
                {
                    map[prop.Name] = prop.Value.ValueKind == JsonValueKind.String
                        ? prop.Value.GetString() ?? string.Empty
                        : prop.Value.GetRawText();
                }
            }
        }
        catch (JsonException)
        {
            // Fall through with whatever parsed — callers treat missing keys
            // as absent config, matching the Rust serde behaviour.
        }

        return map;
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

    /// <summary>
    /// Full-row DTO for the create response — the complete
    /// <c>ProjectTesterRow</c> shape the Rust <c>create_tester</c> returns
    /// (same fields as <c>SELECT_COLUMNS</c> / the list_testers rows).
    /// </summary>
    private static object ToFullDto(ProjectTester t) => new
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
        avg_benchmark_duration_seconds = t.AvgBenchmarkDurationSeconds,
        benchmark_run_count = t.BenchmarkRunCount,
        created_by = t.CreatedBy,
        created_at = t.CreatedAt,
        updated_at = t.UpdatedAt,
        cloud_connection_id = t.CloudConnectionId,
        cloud_account_id = t.CloudAccountId,
        requested_os = t.RequestedOs,
        requested_variant = t.RequestedVariant,
        os_distro = t.OsDistro,
        os_version = t.OsVersion,
        os_variant = t.OsVariant,
        os_arch = t.OsArch,
        os_kernel = t.OsKernel,
    };

    // ── Request bodies (snake_case via [FromBody] + JSON property names) ──────

    /// <summary>
    /// Body for POST /testers — mirrors the Rust <c>CreateTesterBody</c>
    /// (dashboard/src/api/testers.ts <c>CreateTesterBody</c> on the wire).
    /// </summary>
    public sealed record CreateTesterBody
    {
        [System.Text.Json.Serialization.JsonPropertyName("name")]
        public string Name { get; init; } = string.Empty;

        [System.Text.Json.Serialization.JsonPropertyName("cloud")]
        public string Cloud { get; init; } = string.Empty;

        [System.Text.Json.Serialization.JsonPropertyName("region")]
        public string Region { get; init; } = string.Empty;

        [System.Text.Json.Serialization.JsonPropertyName("vm_size")]
        public string? VmSize { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("auto_shutdown_local_hour")]
        public short? AutoShutdownLocalHour { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("auto_probe_enabled")]
        public bool? AutoProbeEnabled { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("cloud_connection_id")]
        public Guid? CloudConnectionId { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("cloud_account_id")]
        public Guid? CloudAccountId { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("requested_os")]
        public string? RequestedOs { get; init; }

        [System.Text.Json.Serialization.JsonPropertyName("requested_variant")]
        public string? RequestedVariant { get; init; }
    }

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
