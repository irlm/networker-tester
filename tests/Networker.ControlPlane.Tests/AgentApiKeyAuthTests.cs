using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Provisioning;
using Networker.ControlPlane.Realtime;
using Networker.ControlPlane.Realtime.RawWs;
using Networker.ControlPlane.Security;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// V040 agent api-key hardening: keys are stored hashed
/// (<c>agent.api_key_hash</c> = lowercase-hex SHA-256) and
/// <see cref="AgentMessageProcessor.AuthenticateAsync"/> resolves agents by
/// that hash with a constant-time digest compare — the plaintext column is
/// never consulted. These tests pin the digest format (it must match the SQL
/// backfill in V040), the fixed-time comparison semantics, and the
/// hash-only lookup behavior end-to-end against a relational (Sqlite)
/// <see cref="NetworkerDbContext"/>.
/// </summary>
public sealed class AgentApiKeyAuthTests
{
    // ── AgentApiKeys.HashHex ─────────────────────────────────────────────────

    [Fact]
    public void HashHex_is_lowercase_hex_sha256()
    {
        // FIPS-180 reference vector: SHA-256("abc").
        Assert.Equal(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            AgentApiKeys.HashHex("abc"));
    }

    [Fact]
    public void HashHex_is_deterministic_and_key_sensitive()
    {
        var key = TesterCreateLogic.GenerateAgentApiKey();

        Assert.Equal(AgentApiKeys.HashHex(key), AgentApiKeys.HashHex(key));
        Assert.NotEqual(AgentApiKeys.HashHex(key), AgentApiKeys.HashHex(key + "x"));
        Assert.Matches("^[0-9a-f]{64}$", AgentApiKeys.HashHex(key));
    }

    // ── AgentApiKeys.FixedTimeEqualsHex ──────────────────────────────────────

    [Theory]
    [InlineData("aa", "aa", true)]
    [InlineData("aa", "ab", false)]
    [InlineData("aa", "aaa", false)] // length mismatch → false, no throw
    [InlineData("", "", true)]
    public void FixedTimeEqualsHex_compares_content(string a, string b, bool expected)
    {
        Assert.Equal(expected, AgentApiKeys.FixedTimeEqualsHex(a, b));
    }

    [Fact]
    public void FixedTimeEqualsHex_rejects_null_without_throwing()
    {
        Assert.False(AgentApiKeys.FixedTimeEqualsHex(null, "aa"));
        Assert.False(AgentApiKeys.FixedTimeEqualsHex("aa", null));
        Assert.False(AgentApiKeys.FixedTimeEqualsHex(null, null));
    }

    // ── AuthenticateAsync (hash-only lookup) ─────────────────────────────────

    [Fact]
    public async Task Authenticate_resolves_agent_by_key_hash()
    {
        using var host = BuildHost();
        var key = TesterCreateLogic.GenerateAgentApiKey();
        var agentId = SeedAgent(host, key, hash: AgentApiKeys.HashHex(key));

        var identity = await Processor(host).AuthenticateAsync(key);

        Assert.NotNull(identity);
        Assert.Equal(agentId, identity!.AgentId);
    }

    [Fact]
    public async Task Authenticate_rejects_unknown_missing_and_empty_keys()
    {
        using var host = BuildHost();
        var key = TesterCreateLogic.GenerateAgentApiKey();
        SeedAgent(host, key, hash: AgentApiKeys.HashHex(key));

        Assert.Null(await Processor(host).AuthenticateAsync("not-the-key"));
        Assert.Null(await Processor(host).AuthenticateAsync(null));
        Assert.Null(await Processor(host).AuthenticateAsync(""));
    }

    [Fact]
    public async Task Authenticate_never_consults_the_plaintext_column()
    {
        // A row whose plaintext matches but whose hash is NULL (pre-backfill
        // shape) must NOT authenticate: the lookup is hash-only by design, so
        // this proves the plaintext column has left the auth path entirely.
        using var host = BuildHost();
        var key = TesterCreateLogic.GenerateAgentApiKey();
        SeedAgent(host, key, hash: null);

        Assert.Null(await Processor(host).AuthenticateAsync(key));
    }

    // ── V044: expiry enforcement ─────────────────────────────────────────────

    [Fact]
    public async Task Authenticate_accepts_a_key_with_null_expiry()
    {
        // NULL expiry = no expiry: the back-compat default for the whole fleet.
        using var host = BuildHost();
        var key = TesterCreateLogic.GenerateAgentApiKey();
        SeedAgent(host, key, hash: AgentApiKeys.HashHex(key), expiresAt: null);

        Assert.NotNull(await Processor(host).AuthenticateAsync(key));
    }

    [Fact]
    public async Task Authenticate_accepts_a_not_yet_expired_key()
    {
        using var host = BuildHost();
        var key = TesterCreateLogic.GenerateAgentApiKey();
        SeedAgent(host, key, hash: AgentApiKeys.HashHex(key),
            expiresAt: DateTime.UtcNow.AddHours(1));

        Assert.NotNull(await Processor(host).AuthenticateAsync(key));
    }

    [Fact]
    public async Task Authenticate_rejects_an_expired_key()
    {
        using var host = BuildHost();
        var key = TesterCreateLogic.GenerateAgentApiKey();
        SeedAgent(host, key, hash: AgentApiKeys.HashHex(key),
            expiresAt: DateTime.UtcNow.AddMinutes(-1));

        Assert.Null(await Processor(host).AuthenticateAsync(key));
    }

    // ── V044: throttled last-used stamping ───────────────────────────────────

    [Fact]
    public async Task StampApiKeyUsed_records_time_and_ip_when_never_stamped()
    {
        using var host = BuildHost();
        var key = TesterCreateLogic.GenerateAgentApiKey();
        var agentId = SeedAgent(host, key, hash: AgentApiKeys.HashHex(key));

        await Processor(host).StampApiKeyUsedAsync(agentId, "203.0.113.7");

        var db = host.GetRequiredService<NetworkerDbContext>();
        db.ChangeTracker.Clear();
        var row = await db.Agents.AsNoTracking().FirstAsync(a => a.AgentId == agentId);
        Assert.NotNull(row.ApiKeyLastUsedAt);
        Assert.Equal("203.0.113.7", row.ApiKeyLastUsedIp);
    }

    [Fact]
    public async Task StampApiKeyUsed_is_throttled_within_the_window()
    {
        using var host = BuildHost();
        var key = TesterCreateLogic.GenerateAgentApiKey();
        var agentId = SeedAgent(host, key, hash: AgentApiKeys.HashHex(key));

        // A recent stamp (2 min ago) is inside the 5-min throttle window, so a
        // fresh stamp attempt must be a no-op — neither time nor IP changes.
        var recent = DateTime.UtcNow.AddMinutes(-2);
        var db = host.GetRequiredService<NetworkerDbContext>();
        await db.Agents.Where(a => a.AgentId == agentId)
            .ExecuteUpdateAsync(s => s
                .SetProperty(a => a.ApiKeyLastUsedAt, recent)
                .SetProperty(a => a.ApiKeyLastUsedIp, "198.51.100.1"));
        db.ChangeTracker.Clear();

        await Processor(host).StampApiKeyUsedAsync(agentId, "203.0.113.7");

        db.ChangeTracker.Clear();
        var row = await db.Agents.AsNoTracking().FirstAsync(a => a.AgentId == agentId);
        Assert.Equal("198.51.100.1", row.ApiKeyLastUsedIp); // unchanged (throttled)
    }

    // ── V044: rotation contract ──────────────────────────────────────────────

    [Fact]
    public async Task Rotation_replaces_the_hash_so_the_old_key_dies_and_the_new_one_works()
    {
        using var host = BuildHost();
        var oldKey = TesterCreateLogic.GenerateAgentApiKey();
        var agentId = SeedAgent(host, oldKey, hash: AgentApiKeys.HashHex(oldKey),
            expiresAt: DateTime.UtcNow.AddMinutes(-1)); // pre-rotation: expired

        // Old (expired) key does not authenticate.
        Assert.Null(await Processor(host).AuthenticateAsync(oldKey));

        // Rotate: exactly what RotateAgentKey persists — new key + hash, expiry
        // cleared.
        var newKey = TesterCreateLogic.GenerateAgentApiKey();
        var db = host.GetRequiredService<NetworkerDbContext>();
        await db.Agents.Where(a => a.AgentId == agentId)
            .ExecuteUpdateAsync(s => s
                .SetProperty(a => a.ApiKey, newKey)
                .SetProperty(a => a.ApiKeyHash, AgentApiKeys.HashHex(newKey))
                .SetProperty(a => a.ApiKeyExpiresAt, (DateTime?)null));
        db.ChangeTracker.Clear();

        // Old key stays dead; new key authenticates.
        Assert.Null(await Processor(host).AuthenticateAsync(oldKey));
        var identity = await Processor(host).AuthenticateAsync(newKey);
        Assert.NotNull(identity);
        Assert.Equal(agentId, identity!.AgentId);
    }

    // ── Test host wiring (same pattern as RunDispatcherTesterFkTests) ────────

    private static ServiceProvider BuildHost()
    {
        var conn = new Microsoft.Data.Sqlite.SqliteConnection("DataSource=:memory:");
        conn.Open();

        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
        services.AddSignalR();
        services.AddDashboardEventBus(); // EventBus for AgentMessageProcessor
        services.AddSingleton(conn);
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(conn));

        var sp = services.BuildServiceProvider();

        // Only the agent table (with its V040 column) is touched by
        // AuthenticateAsync; the full Postgres model can't build on Sqlite.
        using var cmd = conn.CreateCommand();
        cmd.CommandText = """
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
                api_key TEXT NOT NULL,
                api_key_hash TEXT,
                api_key_expires_at TEXT,
                api_key_last_used_at TEXT,
                api_key_last_used_ip TEXT,
                tags TEXT,
                project_id TEXT NOT NULL,
                tester_id TEXT
            );
            """;
        cmd.ExecuteNonQuery();
        return sp;
    }

    private static AgentMessageProcessor Processor(IServiceProvider sp) => new(
        sp.GetRequiredService<NetworkerDbContext>(),
        sp.GetRequiredService<EventBus>(),
        sp.GetRequiredService<ILogger<AgentMessageProcessor>>());

    private static Guid SeedAgent(IServiceProvider sp, string apiKey, string? hash, DateTime? expiresAt = null)
    {
        var db = sp.GetRequiredService<NetworkerDbContext>();
        var id = Guid.NewGuid();
        db.Agents.Add(new Agent
        {
            AgentId = id,
            Name = $"agent-{id:N}",
            Status = "offline",
            ApiKey = apiKey,
            ApiKeyHash = hash,
            ApiKeyExpiresAt = expiresAt,
            ProjectId = "proj-apikey-test",
            RegisteredAt = DateTime.UtcNow,
        });
        db.SaveChanges();
        db.ChangeTracker.Clear();
        return id;
    }
}
