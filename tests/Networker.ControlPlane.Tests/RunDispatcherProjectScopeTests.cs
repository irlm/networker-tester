using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Dispatch;
using Networker.ControlPlane.Realtime;
using Networker.Data;
using Networker.Data.Entities;
using Networker.Security;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// P0 regression guard for the project-isolation audit §4: run dispatch must be
/// project-scoped so a project-A <c>sdkprobe</c> run — which ships project A's
/// DECRYPTED LagHound customer token in the <c>assign_run</c> payload — is
/// NEVER dispatched to an agent bound to a different project. Before the fix,
/// <c>SelectTargetAgentAsync</c> fell through to any online, version-compatible
/// agent regardless of project, leaking the plaintext token to another tenant's
/// machine.
///
/// <para>These tests wire a real EF-Core-Sqlite <see cref="NetworkerDbContext"/>
/// (relational, because the production paths use <c>ExecuteUpdateAsync</c>) with
/// TWO projects and record the exact wire frame each agent's registered sender
/// receives, so we can assert both routing (which agent, if any) and payload
/// contents (does the plaintext token appear).</para>
/// </summary>
public sealed class RunDispatcherProjectScopeTests
{
    private const string ProjectA = "proj-aaaaaaa1";
    private const string ProjectB = "proj-bbbbbbb2";

    // The plaintext SDK token we encrypt into project A's config. It must NEVER
    // appear in any frame sent to a project-B agent.
    private const string ProjectATokenPlaintext = "lh_secret_token_for_project_A_only";

    // A fixed 32-byte key so encrypt-at-seed and decrypt-at-dispatch agree.
    private static readonly byte[] CipherKey = Enumerable.Range(0, CredentialCipher.KeySize)
        .Select(i => (byte)(i + 1)).ToArray();

    private static CredentialCipher Cipher() => new(CipherKey);

    // ── Test host wiring (mirrors RunDispatcherTesterFkTests) ────────────────

    private static ServiceProvider BuildHost()
    {
        var conn = new Microsoft.Data.Sqlite.SqliteConnection("DataSource=:memory:");
        conn.Open();

        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
        services.AddSignalR();
        services.AddAgentProtocol();
        services.AddDashboardEventBus();
        services.AddSingleton(conn);
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(conn));

        var sp = services.BuildServiceProvider();
        CreateMinimalSchema(conn);
        return sp;
    }

    private static void CreateMinimalSchema(Microsoft.Data.Sqlite.SqliteConnection conn)
    {
        Exec(conn, "PRAGMA foreign_keys = ON;");
        Exec(conn, """
            CREATE TABLE project (
                project_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                slug TEXT NOT NULL,
                description TEXT,
                created_by TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                settings TEXT NOT NULL,
                deleted_at TEXT,
                delete_protection INTEGER NOT NULL DEFAULT 0
            );
            """);
        Exec(conn, """
            CREATE TABLE project_tester (
                tester_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                name TEXT NOT NULL,
                cloud TEXT NOT NULL,
                region TEXT NOT NULL,
                vm_size TEXT NOT NULL,
                vm_name TEXT,
                vm_resource_id TEXT,
                public_ip TEXT,
                ssh_user TEXT NOT NULL,
                power_state TEXT NOT NULL,
                allocation TEXT NOT NULL,
                status_message TEXT,
                locked_by_config_id TEXT,
                installer_version TEXT,
                last_installed_at TEXT,
                auto_shutdown_enabled INTEGER NOT NULL DEFAULT 0,
                auto_shutdown_local_hour INTEGER NOT NULL DEFAULT 0,
                next_shutdown_at TEXT,
                shutdown_deferral_count INTEGER NOT NULL DEFAULT 0,
                auto_probe_enabled INTEGER NOT NULL DEFAULT 0,
                last_used_at TEXT,
                avg_benchmark_duration_seconds INTEGER,
                benchmark_run_count INTEGER NOT NULL DEFAULT 0,
                created_by TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                cloud_connection_id TEXT,
                requested_os TEXT,
                requested_variant TEXT,
                os_distro TEXT,
                os_version TEXT,
                os_variant TEXT,
                os_arch TEXT,
                os_kernel TEXT,
                cloud_account_id TEXT
            );
            """);
        Exec(conn, """
            CREATE TABLE agent (
                agent_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                region TEXT,
                provider TEXT,
                status TEXT NOT NULL,
                version TEXT,
                os TEXT,
                arch TEXT,
                last_heartbeat TEXT,
                registered_at TEXT NOT NULL,
                api_key_hash TEXT,
                api_key_expires_at TEXT,
                api_key_last_used_at TEXT,
                api_key_last_used_ip TEXT,
                tags TEXT,
                project_id TEXT NOT NULL,
                tester_id TEXT REFERENCES project_tester(tester_id)
            );
            """);
        Exec(conn, """
            CREATE TABLE test_config (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT,
                endpoint_kind TEXT NOT NULL,
                endpoint_ref TEXT NOT NULL,
                workload TEXT NOT NULL,
                methodology TEXT,
                created_by TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                baseline_run_id TEXT,
                max_duration_secs INTEGER NOT NULL DEFAULT 0,
                token_enc BLOB,
                token_nonce BLOB
            );
            """);
        Exec(conn, """
            CREATE TABLE test_run (
                id TEXT PRIMARY KEY,
                test_config_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                status TEXT NOT NULL,
                started_at TEXT,
                finished_at TEXT,
                success_count INTEGER NOT NULL DEFAULT 0,
                failure_count INTEGER NOT NULL DEFAULT 0,
                error_message TEXT,
                artifact_id TEXT,
                tester_id TEXT
                    CONSTRAINT test_run_tester_id_fkey REFERENCES project_tester(tester_id),
                worker_id TEXT,
                last_heartbeat TEXT,
                created_at TEXT NOT NULL,
                comparison_group_id TEXT,
                provisioning_deployment_id TEXT
            );
            """);
    }

    private static void Exec(Microsoft.Data.Sqlite.SqliteConnection conn, string sql)
    {
        using var cmd = conn.CreateCommand();
        cmd.CommandText = sql;
        cmd.ExecuteNonQuery();
    }

    private static NetworkerDbContext Db(IServiceProvider sp)
        => sp.GetRequiredService<NetworkerDbContext>();

    private static void SeedProject(NetworkerDbContext db, string projectId)
    {
        var now = DateTime.UtcNow;
        db.Projects.Add(new Project
        {
            ProjectId = projectId,
            Name = projectId,
            Slug = projectId,
            Settings = "{}",
            CreatedAt = now,
            UpdatedAt = now,
        });
    }

    private static Guid SeedAgent(NetworkerDbContext db, string projectId, string version = "0.28.10")
    {
        var id = Guid.NewGuid();
        db.Agents.Add(new Agent
        {
            AgentId = id,
            Name = $"agent-{id:N}",
            Status = "online",
            Version = version,
            ProjectId = projectId,
            RegisteredAt = DateTime.UtcNow,
            TesterId = null,
        });
        return id;
    }

    /// <summary>Seed a sdkprobe config in <paramref name="projectId"/> whose
    /// stored token encrypts <paramref name="plaintext"/> under the test key.</summary>
    private static Guid SeedSdkConfig(NetworkerDbContext db, string projectId, string plaintext)
    {
        var (enc, nonce) = Cipher().Encrypt(System.Text.Encoding.UTF8.GetBytes(plaintext));
        var id = Guid.NewGuid();
        db.TestConfigs.Add(new TestConfig
        {
            Id = id,
            ProjectId = projectId,
            Name = "sdk-cfg",
            EndpointKind = "network",
            EndpointRef = """{"kind":"network","host":"127.0.0.1","port":443}""",
            // A sdkprobe workload — modes must contain "sdkprobe" for the token splice.
            Workload = """{"modes":["sdkprobe"],"insecure":false}""",
            CreatedAt = DateTime.UtcNow,
            UpdatedAt = DateTime.UtcNow,
            TokenEnc = enc,
            TokenNonce = nonce,
        });
        return id;
    }

    private static AuthUser Caller() => new(
        UserId: Guid.NewGuid(),
        Email: "t@example.com",
        Role: "admin",
        IsPlatformAdmin: true);

    private static RunDispatcher Dispatcher(IServiceProvider sp, NetworkerDbContext db)
        => new(
            db,
            sp.GetRequiredService<AgentConnectionRegistry>(),
            sp.GetRequiredService<EventBus>(),
            sp.GetRequiredService<ILogger<RunDispatcher>>(),
            Cipher());

    /// <summary>Register an agent whose sender records every frame it is sent.</summary>
    private static void RegisterRecording(
        AgentConnectionRegistry registry, Guid agentId, System.Collections.Concurrent.ConcurrentQueue<string> sink)
    {
        registry.Register(agentId, $"raw-{agentId}", (payload, _) =>
        {
            sink.Enqueue(payload);
            return Task.CompletedTask;
        });
    }

    // ── 1. P0: cross-project run does NOT dispatch to a foreign-project agent ──

    [Fact]
    public async Task ProjectA_sdkprobe_run_never_dispatched_to_projectB_agent_and_token_never_leaks()
    {
        using var sp = BuildHost();
        var db = Db(sp);
        var registry = sp.GetRequiredService<AgentConnectionRegistry>();

        SeedProject(db, ProjectA);
        SeedProject(db, ProjectB);
        // ONLY a project-B agent is online. Project A has NO online agent.
        var agentB = SeedAgent(db, ProjectB);
        var configA = SeedSdkConfig(db, ProjectA, ProjectATokenPlaintext);
        await db.SaveChangesAsync();

        var bFrames = new System.Collections.Concurrent.ConcurrentQueue<string>();
        RegisterRecording(registry, agentB, bFrames);

        var runId = await Dispatcher(sp, db).LaunchAsync(configA, null, null, Caller(), default);

        var run = await db.TestRuns.AsNoTracking().FirstAsync(r => r.Id == runId);

        // The run must NOT have been assigned: no worker stamped, still queued.
        Assert.Null(run.WorkerId);
        Assert.Equal("queued", run.Status);

        // The project-B agent must have received NOTHING — no assign_run at all,
        // and therefore the project-A plaintext token cannot have crossed over.
        Assert.Empty(bFrames);
        Assert.DoesNotContain(bFrames, f => f.Contains(ProjectATokenPlaintext));
    }

    // ── 2. Same-project agent DOES receive the run + the token ────────────────

    [Fact]
    public async Task ProjectA_sdkprobe_run_dispatches_to_projectA_agent_with_token()
    {
        using var sp = BuildHost();
        var db = Db(sp);
        var registry = sp.GetRequiredService<AgentConnectionRegistry>();

        SeedProject(db, ProjectA);
        SeedProject(db, ProjectB);
        var agentA = SeedAgent(db, ProjectA);
        var agentB = SeedAgent(db, ProjectB); // foreign agent also online — must be ignored
        var configA = SeedSdkConfig(db, ProjectA, ProjectATokenPlaintext);
        await db.SaveChangesAsync();

        var aFrames = new System.Collections.Concurrent.ConcurrentQueue<string>();
        var bFrames = new System.Collections.Concurrent.ConcurrentQueue<string>();
        RegisterRecording(registry, agentA, aFrames);
        RegisterRecording(registry, agentB, bFrames);

        var runId = await Dispatcher(sp, db).LaunchAsync(configA, null, null, Caller(), default);

        var run = await db.TestRuns.AsNoTracking().FirstAsync(r => r.Id == runId);

        // Assigned to the SAME-project agent.
        Assert.Equal(agentA.ToString(), run.WorkerId);

        // Project A's agent got exactly one assign_run carrying the plaintext token.
        Assert.Single(aFrames);
        Assert.True(aFrames.TryPeek(out var aFrame));
        Assert.Contains(ProjectATokenPlaintext, aFrame);
        Assert.Contains("laghound_token", aFrame);

        // The foreign agent got nothing.
        Assert.Empty(bFrames);
    }

    // ── 3. Config in a different project than its run → token withheld ────────

    [Fact]
    public async Task Token_withheld_when_config_project_differs_from_run_project()
    {
        // Directly exercise the SerializeForAssign defense-in-depth guard: an
        // online same-project agent IS selected (so dispatch proceeds), but the
        // config's project disagrees with the run's project (an invariant
        // violation). The guard must refuse to attach the token — the run is
        // still assigned, but WITHOUT the plaintext token in the frame.
        using var sp = BuildHost();
        var db = Db(sp);
        var registry = sp.GetRequiredService<AgentConnectionRegistry>();

        SeedProject(db, ProjectA);
        SeedProject(db, ProjectB);
        // Agent + run in project A (so selection succeeds), but the sdkprobe
        // config is (wrongly) in project B.
        var agentA = SeedAgent(db, ProjectA);
        var configB = SeedSdkConfig(db, ProjectB, ProjectATokenPlaintext);

        var runId = Guid.NewGuid();
        db.TestRuns.Add(new TestRun
        {
            Id = runId,
            TestConfigId = configB,   // config belongs to B …
            ProjectId = ProjectA,     // … but the run is in A (the mismatch)
            Status = "queued",
            CreatedAt = DateTime.UtcNow,
        });
        await db.SaveChangesAsync();

        var aFrames = new System.Collections.Concurrent.ConcurrentQueue<string>();
        RegisterRecording(registry, agentA, aFrames);

        await Dispatcher(sp, db).DispatchAsync(runId, default);

        var run = await db.TestRuns.AsNoTracking().FirstAsync(r => r.Id == runId);

        // The run WAS assigned to project A's agent (selection is by run.project).
        Assert.Equal(agentA.ToString(), run.WorkerId);
        // But the guard refused the token: the frame carries no plaintext token.
        Assert.Single(aFrames);
        Assert.True(aFrames.TryPeek(out var frame));
        Assert.DoesNotContain(ProjectATokenPlaintext, frame);
        Assert.DoesNotContain("laghound_token", frame);
    }

    // ── 4. Redispatch is project-scoped too ───────────────────────────────────

    [Fact]
    public async Task Redispatch_does_not_route_projectA_run_to_projectB_agent()
    {
        using var sp = BuildHost();
        var db = Db(sp);
        var registry = sp.GetRequiredService<AgentConnectionRegistry>();

        SeedProject(db, ProjectA);
        SeedProject(db, ProjectB);
        var agentB = SeedAgent(db, ProjectB);
        var configA = SeedSdkConfig(db, ProjectA, ProjectATokenPlaintext);

        // A queued project-A run older than QUEUED_MIN_AGE_SECS so the
        // redispatcher considers it.
        var runId = Guid.NewGuid();
        db.TestRuns.Add(new TestRun
        {
            Id = runId,
            TestConfigId = configA,
            ProjectId = ProjectA,
            Status = "queued",
            CreatedAt = DateTime.UtcNow.AddMinutes(-5),
        });
        await db.SaveChangesAsync();

        var bFrames = new System.Collections.Concurrent.ConcurrentQueue<string>();
        RegisterRecording(registry, agentB, bFrames);

        var dispatched = await Dispatcher(sp, db).RedispatchQueuedAsync(default);

        Assert.Equal(0, dispatched);
        var run = await db.TestRuns.AsNoTracking().FirstAsync(r => r.Id == runId);
        Assert.Equal("queued", run.Status);
        Assert.Null(run.WorkerId);
        Assert.Empty(bFrames);
    }
}
