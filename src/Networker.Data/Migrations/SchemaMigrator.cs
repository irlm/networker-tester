using Npgsql;

namespace Networker.Data.Migrations;

/// <summary>Outcome of a <see cref="SchemaMigrator.MigrateAsync(string, CancellationToken)"/> call.</summary>
/// <param name="Applied">Versions applied by this call, in order.</param>
/// <param name="AlreadyApplied">Versions that were already recorded in <c>_migrations</c>.</param>
public sealed record SchemaMigrationResult(IReadOnlyList<int> Applied, IReadOnlyList<int> AlreadyApplied)
{
    /// <summary>True when the database was already fully migrated.</summary>
    public bool WasUpToDate => Applied.Count == 0;
}

/// <summary>
/// The schema owner for the Networker control-plane PostgreSQL database.
///
/// This is the C# replacement for the legacy Rust migration runner
/// (<c>crates/networker-dashboard/src/db/migrations.rs</c>, deleted with the
/// Rust decommission). It applies the same migration chain (V002…V039; V001
/// is the networker-tester probe-result schema, owned by the tester crate)
/// and uses the SAME bookkeeping table — <c>_migrations (version INT PRIMARY
/// KEY, applied_at TIMESTAMPTZ)</c> — with one row per applied version. That
/// guarantees:
/// <list type="bullet">
///   <item>an existing production database already migrated by the Rust
///   runner reports zero pending migrations here, and</item>
///   <item>a fresh database replays the exact chain the Rust runner would
///   have replayed, producing an identical schema.</item>
/// </list>
///
/// Scripts V002…V039 are verbatim copies of the Rust runner's SQL, embedded
/// as resources under <c>Migrations/</c>. V025 (UUID → base36 project ids)
/// was code in Rust and is code here — see <see cref="V025ProjectIdMigration"/>.
///
/// Historical scripts are FROZEN: never edit them. New schema changes are new
/// <c>V0NN_*.sql</c> files (see <c>docs/schema-ownership.md</c>).
/// </summary>
public static class SchemaMigrator
{
    /// <summary>The bookkeeping table shared with the legacy Rust runner.</summary>
    public const string BookkeepingTable = "_migrations";

    /// <summary>First version this runner owns (V001 belongs to networker-tester).</summary>
    public const int FirstVersion = 2;

    /// <summary>Latest known migration version.</summary>
    public const int LatestVersion = 40;

    /// <summary>Versions implemented in C# rather than an embedded SQL script.</summary>
    private static readonly Dictionary<int, Func<NpgsqlConnection, CancellationToken, Task>> CodeMigrations = new()
    {
        [25] = V025ProjectIdMigration.ApplyAsync,
    };

    /// <summary>
    /// Migrations that manage their own transaction (script contains explicit
    /// BEGIN/COMMIT), so the runner must not wrap them in another one.
    /// </summary>
    private static readonly HashSet<int> SelfTransactional = [8];

    /// <summary>All versions this runner knows, in apply order.</summary>
    public static IReadOnlyList<int> KnownVersions { get; } =
        Enumerable.Range(FirstVersion, LatestVersion - FirstVersion + 1).ToArray();

    /// <summary>Session-scoped advisory lock key so concurrent starts serialize.</summary>
    private const long AdvisoryLockKey = 0x6E77_6B72_6D69_6772; // "nwkrmigr"

    /// <summary>Open a connection and apply all pending migrations.</summary>
    public static async Task<SchemaMigrationResult> MigrateAsync(string connectionString, CancellationToken ct = default)
    {
        await using var conn = new NpgsqlConnection(connectionString);
        await conn.OpenAsync(ct);
        return await MigrateAsync(conn, ct);
    }

    /// <summary>Apply all pending migrations on an open connection.</summary>
    public static async Task<SchemaMigrationResult> MigrateAsync(NpgsqlConnection conn, CancellationToken ct = default)
    {
        await ExecAsync(conn, $"SELECT pg_advisory_lock({AdvisoryLockKey})", ct);
        try
        {
            // Same DDL the Rust runner used to bootstrap its bookkeeping.
            await ExecAsync(conn, $"""
                CREATE TABLE IF NOT EXISTS {BookkeepingTable} (
                    version INT NOT NULL PRIMARY KEY,
                    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
                )
                """, ct);

            var recorded = new HashSet<int>();
            await using (var cmd = new NpgsqlCommand($"SELECT version FROM {BookkeepingTable}", conn))
            await using (var reader = await cmd.ExecuteReaderAsync(ct))
            {
                while (await reader.ReadAsync(ct))
                {
                    recorded.Add(reader.GetInt32(0));
                }
            }

            var applied = new List<int>();
            var alreadyApplied = new List<int>();

            foreach (var version in KnownVersions)
            {
                if (recorded.Contains(version))
                {
                    alreadyApplied.Add(version);
                    continue;
                }

                await ApplyOneAsync(conn, version, ct);
                applied.Add(version);
            }

            return new SchemaMigrationResult(applied, alreadyApplied);
        }
        finally
        {
            await ExecAsync(conn, $"SELECT pg_advisory_unlock({AdvisoryLockKey})", ct);
        }
    }

    private static async Task ApplyOneAsync(NpgsqlConnection conn, int version, CancellationToken ct)
    {
        if (CodeMigrations.TryGetValue(version, out var codeMigration))
        {
            // Code migrations run statement-by-statement (like the Rust
            // original, which issued a sequence of batch_execute calls).
            await codeMigration(conn, ct);
            await RecordAsync(conn, version, transaction: null, ct);
            return;
        }

        var script = GetScript(version);

        if (SelfTransactional.Contains(version))
        {
            // The script drives its own BEGIN/COMMIT — run it as-is.
            await ExecAsync(conn, script, ct);
            await RecordAsync(conn, version, transaction: null, ct);
            return;
        }

        await using var tx = await conn.BeginTransactionAsync(ct);
        await using (var cmd = new NpgsqlCommand(script, conn, tx))
        {
            await cmd.ExecuteNonQueryAsync(ct);
        }
        await RecordAsync(conn, version, tx, ct);
        await tx.CommitAsync(ct);
    }

    private static async Task RecordAsync(NpgsqlConnection conn, int version, NpgsqlTransaction? transaction, CancellationToken ct)
    {
        await using var cmd = new NpgsqlCommand(
            $"INSERT INTO {BookkeepingTable} (version) VALUES ($1) ON CONFLICT DO NOTHING", conn, transaction);
        cmd.Parameters.AddWithValue(version);
        await cmd.ExecuteNonQueryAsync(ct);
    }

    /// <summary>
    /// Load the embedded SQL script for <paramref name="version"/>.
    /// Throws for code-based versions (V025) and unknown versions.
    /// </summary>
    public static string GetScript(int version)
    {
        if (CodeMigrations.ContainsKey(version))
        {
            throw new InvalidOperationException(
                $"V{version:D3} is a code migration (see V{version:D3}ProjectIdMigration) — it has no SQL script.");
        }

        var assembly = typeof(SchemaMigrator).Assembly;
        var prefix = $"Networker.Data.Migrations.V{version:D3}_";
        var name = assembly.GetManifestResourceNames()
            .SingleOrDefault(n => n.StartsWith(prefix, StringComparison.Ordinal) && n.EndsWith(".sql", StringComparison.Ordinal))
            ?? throw new InvalidOperationException($"No embedded migration script found for V{version:D3} (prefix {prefix}).");

        using var stream = assembly.GetManifestResourceStream(name)!;
        using var readerStream = new StreamReader(stream);
        return readerStream.ReadToEnd();
    }

    /// <summary>Resource file names of every embedded script, in version order (test hook).</summary>
    public static IReadOnlyList<string> ScriptResourceNames() =>
        typeof(SchemaMigrator).Assembly.GetManifestResourceNames()
            .Where(n => n.Contains(".Migrations.V") && n.EndsWith(".sql", StringComparison.Ordinal))
            .OrderBy(n => n, StringComparer.Ordinal)
            .ToArray();

    private static async Task ExecAsync(NpgsqlConnection conn, string sql, CancellationToken ct)
    {
        await using var cmd = new NpgsqlCommand(sql, conn);
        await cmd.ExecuteNonQueryAsync(ct);
    }
}
