using Microsoft.EntityFrameworkCore;
using Networker.Data;
using Networker.Data.Migrations;
using Npgsql;
using Testcontainers.PostgreSql;

namespace Networker.Tests;

/// <summary>
/// Proof that schema ownership has moved to Networker.Data: the ported
/// migration chain (V002..V039, verbatim from the deleted Rust runner in
/// crates/networker-dashboard/src/db/migrations.rs) builds a fresh database
/// that the reverse-engineered EF model can query. Because the EF model was
/// scaffolded FROM the real production schema (which the Rust runner built),
/// "every mapped entity queries cleanly" is the strongest available proxy for
/// schema equivalence.
///
/// Also proves bookkeeping compatibility: the migrator uses the same
/// <c>_migrations (version, applied_at)</c> table, so a production database
/// already migrated by Rust reports zero pending work.
/// </summary>
public sealed class SchemaMigrationFixture : IAsyncLifetime
{
    private readonly PostgreSqlContainer _db = new PostgreSqlBuilder()
        .WithImage("postgres:16-alpine")
        .WithDatabase("networker_schema")
        .WithUsername("networker")
        .WithPassword("networker")
        .Build();

    public string ConnectionString => _db.GetConnectionString();

    /// <summary>Result of the first (fresh-database) migration pass.</summary>
    public SchemaMigrationResult FreshRun { get; private set; } = null!;

    public async Task InitializeAsync()
    {
        await _db.StartAsync();
        FreshRun = await SchemaMigrator.MigrateAsync(ConnectionString);
    }

    public async Task DisposeAsync() => await _db.DisposeAsync();

    public NetworkerDbContext NewDbContext() =>
        new(new DbContextOptionsBuilder<NetworkerDbContext>().UseNpgsql(ConnectionString).Options);
}

public sealed class SchemaMigrationTests : IClassFixture<SchemaMigrationFixture>
{
    private readonly SchemaMigrationFixture _fx;

    public SchemaMigrationTests(SchemaMigrationFixture fx) => _fx = fx;

    // ── Migration chain ─────────────────────────────────────────────────

    [Fact]
    public void Fresh_database_applies_the_full_chain_v002_to_v039()
    {
        Assert.Equal(Enumerable.Range(2, 38), _fx.FreshRun.Applied);
        Assert.Empty(_fx.FreshRun.AlreadyApplied);
    }

    [Fact]
    public async Task Rerun_on_migrated_database_applies_nothing()
    {
        // This is the "existing prod DB migrated by the Rust runner" case:
        // every version is already in _migrations, so nothing is pending.
        var second = await SchemaMigrator.MigrateAsync(_fx.ConnectionString);

        Assert.True(second.WasUpToDate);
        Assert.Empty(second.Applied);
        Assert.Equal(Enumerable.Range(2, 38), second.AlreadyApplied);
    }

    [Fact]
    public async Task Bookkeeping_table_matches_the_rust_runner_contract()
    {
        await using var conn = new NpgsqlConnection(_fx.ConnectionString);
        await conn.OpenAsync();

        // Same table name, same columns, same one-row-per-version content the
        // Rust runner left behind (INT PK version + TIMESTAMPTZ applied_at).
        await using var cmd = new NpgsqlCommand(
            """
            SELECT column_name, data_type
            FROM information_schema.columns
            WHERE table_schema = 'public' AND table_name = '_migrations'
            ORDER BY column_name
            """, conn);
        var columns = new List<(string Name, string Type)>();
        await using (var reader = await cmd.ExecuteReaderAsync())
        {
            while (await reader.ReadAsync())
            {
                columns.Add((reader.GetString(0), reader.GetString(1)));
            }
        }

        Assert.Equal(
            new[] { ("applied_at", "timestamp with time zone"), ("version", "integer") },
            columns);

        await using var versions = new NpgsqlCommand(
            "SELECT version FROM _migrations ORDER BY version", conn);
        var recorded = new List<int>();
        await using (var reader = await versions.ExecuteReaderAsync())
        {
            while (await reader.ReadAsync())
            {
                recorded.Add(reader.GetInt32(0));
            }
        }

        Assert.Equal(Enumerable.Range(2, 38), recorded);
    }

    // ── EF-model equivalence ────────────────────────────────────────────

    [Fact]
    public async Task Every_ef_mapped_entity_queries_the_migrated_schema()
    {
        await using var db = _fx.NewDbContext();

        // Materializing each DbSet issues a SELECT over every mapped column —
        // a missing table, missing column, or type mismatch throws here.
        var queried = 0;
        await db.Agents.ToListAsync(); queried++;
        await db.CloudAccounts.ToListAsync(); queried++;
        await db.Projects.ToListAsync(); queried++;
        await db.ProjectTesters.ToListAsync(); queried++;
        await db.TestConfigs.ToListAsync(); queried++;
        await db.TestRuns.ToListAsync(); queried++;
        await db.DashUsers.ToListAsync(); queried++;
        await db.Deployments.ToListAsync(); queried++;
        await db.CloudConnections.ToListAsync(); queried++;
        await db.ProjectMembers.ToListAsync(); queried++;
        await db.ShareLinks.ToListAsync(); queried++;
        await db.CommandApprovals.ToListAsync(); queried++;
        await db.TestVisibilityRules.ToListAsync(); queried++;
        await db.WorkspaceInvites.ToListAsync(); queried++;
        await db.WorkspaceWarnings.ToListAsync(); queried++;
        await db.BenchmarkVmCatalogs.ToListAsync(); queried++;
        await db.SovereigntyZones.ToListAsync(); queried++;
        await db.ServerRegistries.ToListAsync(); queried++;
        await db.ProjectRoutings.ToListAsync(); queried++;
        await db.MigrationRequests.ToListAsync(); queried++;
        await db.MigrationAuditLogs.ToListAsync(); queried++;
        await db.SystemHealths.ToListAsync(); queried++;
        await db.SsoProviders.ToListAsync(); queried++;
        await db.SystemConfigs.ToListAsync(); queried++;
        await db.AgentCommands.ToListAsync(); queried++;
        await db.VmLifecycles.ToListAsync(); queried++;
        await db.CostRates.ToListAsync(); queried++;
        await db.BenchmarkArtifacts.ToListAsync(); queried++;
        await db.TestSchedules.ToListAsync(); queried++;
        await db.ComparisonGroups.ToListAsync(); queried++;

        // If someone adds a DbSet without extending this list, fail loudly so
        // the new entity is covered by the equivalence proof too.
        Assert.Equal(db.Model.GetEntityTypes().Count(), queried);
    }

    [Fact]
    public async Task Migrated_data_matches_the_rust_runner_output()
    {
        await using var db = _fx.NewDbContext();

        // V010 created the Default project; V025 rewrote its id from the
        // well-known UUID to a valid 14-char base36 id (zone us, server a20);
        // V012 flipped delete_protection on.
        var defaultProject = Assert.Single(await db.Projects.ToListAsync());
        Assert.Equal("default", defaultProject.Slug);
        Assert.True(defaultProject.DeleteProtection);
        Assert.Equal(14, defaultProject.ProjectId.Length);
        Assert.True(ProjectId36.Validate(defaultProject.ProjectId),
            $"Default project id '{defaultProject.ProjectId}' fails Damm validation");
        Assert.StartsWith("us", defaultProject.ProjectId);
        Assert.EndsWith("a20", defaultProject.ProjectId[..12]);

        // V025 step 9 routed every project to us/us.
        var routing = Assert.Single(await db.ProjectRoutings.ToListAsync());
        Assert.Equal(defaultProject.ProjectId, routing.ProjectId);
        Assert.Equal("us", routing.HomeZone.Trim());
        Assert.Equal("us", routing.CurrentZone.Trim());

        // V024 seeded 31 sovereignty zones + the a20 server; V034 seeded
        // 17 cost rates (7 aws + 5 azure + 5 gcp).
        Assert.Equal(31, await db.SovereigntyZones.CountAsync());
        Assert.Single(await db.ServerRegistries.ToListAsync());
        Assert.Equal(17, await db.CostRates.CountAsync());
    }

    [Fact]
    public async Task Ef_can_insert_through_the_migrated_schema()
    {
        // A write round-trip through the central tables exercises defaults,
        // CHECK constraints, and the FK graph — closer to prod usage than
        // SELECTs alone.
        var projectId = ProjectId36.Generate("us", "a20");
        var userId = Guid.NewGuid();

        await using (var db = _fx.NewDbContext())
        {
            db.DashUsers.Add(new()
            {
                UserId = userId,
                Email = $"schema-test-{userId:N}@networker.local",
                Role = "admin",
                Status = "active",
            });
            db.Projects.Add(new()
            {
                ProjectId = projectId,
                Name = "schema-equivalence",
                Slug = $"schema-eq-{userId:N}"[..32],
                CreatedBy = userId,
            });
            await db.SaveChangesAsync();

            var config = new Networker.Data.Entities.TestConfig
            {
                ProjectId = projectId,
                Name = "url-probe",
                EndpointKind = "network",
                EndpointRef = """{"host":"example.com"}""",
                Workload = """{"mode":"http1"}""",
                CreatedBy = userId,
            };
            db.TestConfigs.Add(config);
            await db.SaveChangesAsync();

            db.TestRuns.Add(new()
            {
                TestConfigId = config.Id,
                ProjectId = projectId,
                Status = "queued",
            });
            await db.SaveChangesAsync();
        }

        await using (var verify = _fx.NewDbContext())
        {
            var run = Assert.Single(
                await verify.TestRuns.Where(r => r.ProjectId == projectId).ToListAsync());
            Assert.Equal("queued", run.Status);
            Assert.NotEqual(Guid.Empty, run.Id); // gen_random_uuid() default fired
        }
    }
}

/// <summary>
/// Frozen-history guard: the V002..V039 scripts are byte-for-byte copies of
/// the Rust runner's SQL (generated from migrations.rs, then verified against
/// a live Postgres 16). Once a migration has shipped it must never change —
/// databases that already ran it would silently diverge from fresh installs.
/// New schema work = a NEW V0NN file (and a new pin here), never an edit.
/// Runs without Docker, so it guards even where Testcontainers can't run.
/// </summary>
public sealed class MigrationScriptFreezeTests
{
    private static readonly IReadOnlyDictionary<string, string> FrozenSha256 = new Dictionary<string, string>
    {
        ["V002_dashboard.sql"] = "0ff7377571d7878db67eb16fd23da614c39cb73cd39b9628841201540d135090",
        ["V003_deployments.sql"] = "0f1be2b6a8fb8bda66a6bec3ce49dc5ed7c73472671848baf3c2f31e2a3d8f70",
        ["V004_must_change_password.sql"] = "9d51abc88501f9e752bca72e7b9fc202b8fe6a7e01b50bcf19c52cc348cc53fd",
        ["V005_packet_capture.sql"] = "bee383bd6e3be3b92bc267b5697de2fd782cfde9a4bc4979823437c7f37a6dbf",
        ["V006_schedules.sql"] = "caa0d4544fbf6acaf89d2144258d3735f6e0cf93d31c716328d17f90c1fc8f5f",
        ["V007_password_reset.sql"] = "5f3d3080eb9ab8fba57a9cfa0c43ad5858b95ce9479e95c846d2ae27fc8beb90",
        ["V008_email_identity.sql"] = "90872fbbfdcae97e389177028606159495d57ca0917a985d60e35e715bcf7e72",
        ["V009_cloud_connections.sql"] = "3839f507a60ecbcbc6a3234ab8c058985bd9b6531d7fa10060892bbfa7908b06",
        ["V010_multi_project.sql"] = "92720694b67a3fed9334c42b21e31b756c1bb069cfa7b01306eeba459d954b02",
        ["V011_not_null_project_id.sql"] = "3f572435f47f560fa6d6e922bdb51ebadce6e0df2eb4482dd5564a30ad2f6975",
        ["V012_workspace_management.sql"] = "64b0567e6281e15cb9a87e2f53e5d1a4419d5411a0369f246e34a21e6b0c9175",
        ["V013_benchmarks.sql"] = "c9f59d6a1ec7249468ed25df383ab34594cee958f0dbb57fdf719198dd2f7377",
        ["V014_benchmark_compare_presets.sql"] = "741b0b9642d3c919f9b8298440c9c9fe32f7d0a119ab68ff4dab6cc5fa9029b1",
        ["V015_tls_profile_columns.sql"] = "2db8611050f698fe39401bed5af62b8ccfeed0b153fbbd1ab03c4250ad37944d",
        ["V016_benchmark_creation.sql"] = "eee4e8940d76bef88a010b4bd848791c0766e6b82752d501385c808ee46c9b98",
        ["V017_benchmark_run_cell_link.sql"] = "7023535d6ac104ee024e29451f640e29c9e02d4645d9a1bfcc09cb93b5982511",
        ["V018_scheduled_benchmarks_regression.sql"] = "175c8cca775a79f637d734073a8d7391191f05f711b58f5f325fcbb96523ca22",
        ["V019_drop_benchmark_pipeline_fks.sql"] = "3cff3b74a66503a2dbb19de936272a4c35911632cca408137240a36250fcd407",
        ["V020_rename_benchmark_cell_to_testbed.sql"] = "c3da0c659ad17abe382c215d77fec95ca4f6f63dd48b5d3516e5574f5083190d",
        ["V021_benchmark_request_progress.sql"] = "68800248be97bf104ce495ae1f7b46d9729e2fef3ace7e2c7124e7ae59e86146",
        ["V022_application_benchmark.sql"] = "8e97418aa865394da23dbd4f5a4353521a34cbaadca6d9e3c2efae84b12546cd",
        ["V023_perf_log.sql"] = "5984605efd92d57603cf3f75b850a8504c845c3d456dd506b0b6d16277774ed4",
        ["V024_sovereignty_zones.sql"] = "a54d9606a352730f4d5fdd870025d1ea7c899d94e13134b903f299c22e01cb1d",
        ["V026_system_health.sql"] = "4d3475dadad7cc1156abfe6fe12577d63d4ba841a34cfb347ee5b10903618e01",
        ["V027_persistent_testers.sql"] = "73797a5928e429c398a219a3077caff315869d5e5b82195a979c87de2f84d7c5",
        ["V028_dispatcher_index.sql"] = "bcc8fca54335b0f58a026d836d9ebcc85c39dc787ae4f7356ff20b57fea551da",
        ["V029_tester_cloud_conn.sql"] = "9946d9b39e81657bfad389cf00ccf3928e05c4195dbc6f55d42b0b24355f2a0d",
        ["V030_sso_and_member_status.sql"] = "0a200f2c6ab620fee4c422b174e577a65039078ec201dc11a3c9bfca2974b006",
        ["V031_tester_os_info.sql"] = "7c8961f91266fe1889b691b360725ca913f47fe47de1db3f66daf1932aedf6e4",
        ["V032_agent_tester_link.sql"] = "4a0f70ec86892902537991a4eaa5aa0c3d18252b85ff82db265fa42488a17ab8",
        ["V033_agent_command.sql"] = "0b724d152847b67816434b38ae7a2c24a988e73277f64b5381e5e013f0eca8f0",
        ["V034_vm_lifecycle.sql"] = "9862af1994d9842883bc6051ef26a501bfe73b7673c5d1698d89e8def4ad09c2",
        ["V035_tester_created_backfill.sql"] = "a331dbba6d48b064d039aefa63c57920146e49cb21f2449490d8b1be182bb021",
        ["V036_unified_test_config.sql"] = "c8886457d8d1ac909165cb6dd2b6aa22c35531843d70487a30075a16bed22585",
        ["V037_comparison_groups.sql"] = "6c68eced9de3e0bb66be0b4e5c6b9554186dd06c0731dcf22349ee21b73609d1",
        ["V038_pending_endpoints.sql"] = "87353405225e0ca4870eaa9a38eb7abac0d28ce1b0664d6841d92fcea974c07c",
        ["V039_tester_cloud_account.sql"] = "cd88b9ab949cd844f100673754cea0507206ad9afee94e6cd45e95016cb45ece",
    };

    [Fact]
    public void Every_known_version_has_a_script_or_code_migration()
    {
        var scripted = SchemaMigrator.ScriptResourceNames()
            .Select(n => int.Parse(n.Split(".Migrations.V")[1][..3]))
            .ToHashSet();

        foreach (var version in SchemaMigrator.KnownVersions)
        {
            if (version == 25)
            {
                Assert.DoesNotContain(version, scripted); // code migration
                continue;
            }
            Assert.Contains(version, scripted);
        }

        Assert.Equal(37, scripted.Count);
    }

    [Fact]
    public void Shipped_scripts_are_frozen()
    {
        foreach (var resource in SchemaMigrator.ScriptResourceNames())
        {
            var fileName = resource[(resource.IndexOf(".Migrations.", StringComparison.Ordinal) + ".Migrations.".Length)..];
            if (!FrozenSha256.TryGetValue(fileName, out var expected))
            {
                // A brand-new migration: allowed, but must be pinned here in
                // the same PR so it freezes from day one.
                Assert.Fail($"New migration script '{fileName}' has no frozen checksum. " +
                            "Add its SHA-256 to MigrationScriptFreezeTests.FrozenSha256.");
            }

            var version = int.Parse(fileName[1..4]);
            var bytes = System.Text.Encoding.UTF8.GetBytes(SchemaMigrator.GetScript(version));
            var actual = Convert.ToHexStringLower(System.Security.Cryptography.SHA256.HashData(bytes));
            Assert.True(expected == actual,
                $"Migration script '{fileName}' changed after shipping (sha256 {actual}, pinned {expected}). " +
                "Shipped migrations are immutable — add a new V0NN script instead.");
        }
    }

    [Theory]
    // Reference vectors from crates/networker-dashboard/src/project_id.rs tests.
    [InlineData("000000000000", "00")] // all_zeros_check_known
    public void Damm_check_digits_match_the_rust_implementation(string raw, string expected)
    {
        Assert.Equal(expected, ProjectId36.DammBase36Double(raw));
        Assert.True(ProjectId36.VerifyDammBase36Double(raw, expected));
    }

    [Fact]
    public void Project_id_generation_matches_rust_semantics()
    {
        // Deterministic parts: zone prefix, 6-char timestamp from the 2026
        // epoch, server id at chars 9..12; Damm double check at 12..14.
        const long epoch2026 = 1_767_225_600;
        var id = ProjectId36.GenerateDeterministic("eu", "ap1", 1_800_000_000);

        Assert.Equal(14, id.Length);
        Assert.StartsWith("eu", id);
        Assert.Equal("ap1", id[9..12]);
        Assert.True(ProjectId36.Validate(id));

        // Timestamp round-trip: decode chars 2..8 as base36.
        var ts = id[2..8].Aggregate(0L, (acc, c) =>
            acc * 36 + (c <= '9' ? c - '0' : c - 'a' + 10));
        Assert.Equal(1_800_000_000 - epoch2026, ts);

        // Same reference behaviour as Rust's validate_rejects_corrupted test.
        Assert.True(ProjectId36.Validate("00000000000000"));
        Assert.False(ProjectId36.Validate("00000000000001"));
        Assert.False(ProjectId36.Validate(id.ToUpperInvariant()));
        Assert.False(ProjectId36.Validate(id[..13]));
    }
}
