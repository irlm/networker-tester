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

    private static Guid SeedAgent(IServiceProvider sp, string apiKey, string? hash)
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
            ProjectId = "proj-apikey-test",
            RegisteredAt = DateTime.UtcNow,
        });
        db.SaveChanges();
        db.ChangeTracker.Clear();
        return id;
    }
}
