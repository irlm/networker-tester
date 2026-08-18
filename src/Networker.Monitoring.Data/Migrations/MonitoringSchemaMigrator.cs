using Npgsql;

namespace Networker.Monitoring.Data.Migrations;

public sealed record MonitoringMigrationResult(
    IReadOnlyList<int> Applied,
    IReadOnlyList<int> AlreadyApplied)
{
    public bool WasUpToDate => Applied.Count == 0;
}

/// <summary>Owns the schema of the independent monitoring database.</summary>
public static class MonitoringSchemaMigrator
{
    public const string BookkeepingTable = "_monitoring_migrations";
    public const int LatestVersion = 1;
    private const long AdvisoryLockKey = 0x6C61_676D_6F6E_6974; // "lagmonit"

    public static async Task<MonitoringMigrationResult> MigrateAsync(
        string connectionString,
        CancellationToken cancellationToken = default)
    {
        await using var connection = new NpgsqlConnection(connectionString);
        await connection.OpenAsync(cancellationToken);
        return await MigrateAsync(connection, cancellationToken);
    }

    public static async Task<MonitoringMigrationResult> MigrateAsync(
        NpgsqlConnection connection,
        CancellationToken cancellationToken = default)
    {
        await ExecuteAsync(connection, $"SELECT pg_advisory_lock({AdvisoryLockKey})", cancellationToken);
        try
        {
            await ExecuteAsync(connection, $"""
                CREATE TABLE IF NOT EXISTS {BookkeepingTable} (
                    version INT NOT NULL PRIMARY KEY,
                    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
                )
                """, cancellationToken);

            var recorded = new HashSet<int>();
            await using (var command = new NpgsqlCommand(
                $"SELECT version FROM {BookkeepingTable}", connection))
            await using (var reader = await command.ExecuteReaderAsync(cancellationToken))
            {
                while (await reader.ReadAsync(cancellationToken))
                {
                    recorded.Add(reader.GetInt32(0));
                }
            }

            var applied = new List<int>();
            var alreadyApplied = new List<int>();
            for (var version = 1; version <= LatestVersion; version++)
            {
                if (recorded.Contains(version))
                {
                    alreadyApplied.Add(version);
                    continue;
                }

                await using var transaction = await connection.BeginTransactionAsync(cancellationToken);
                await using (var migration = new NpgsqlCommand(GetScript(version), connection, transaction))
                {
                    await migration.ExecuteNonQueryAsync(cancellationToken);
                }

                await using (var record = new NpgsqlCommand(
                    $"INSERT INTO {BookkeepingTable} (version) VALUES ($1)", connection, transaction))
                {
                    record.Parameters.AddWithValue(version);
                    await record.ExecuteNonQueryAsync(cancellationToken);
                }

                await transaction.CommitAsync(cancellationToken);
                applied.Add(version);
            }

            return new MonitoringMigrationResult(applied, alreadyApplied);
        }
        finally
        {
            await ExecuteAsync(connection, $"SELECT pg_advisory_unlock({AdvisoryLockKey})", cancellationToken);
        }
    }

    public static string GetScript(int version)
    {
        var prefix = $"Networker.Monitoring.Data.Migrations.V{version:D3}_";
        var assembly = typeof(MonitoringSchemaMigrator).Assembly;
        var resourceName = assembly.GetManifestResourceNames()
            .SingleOrDefault(name => name.StartsWith(prefix, StringComparison.Ordinal)
                && name.EndsWith(".sql", StringComparison.Ordinal))
            ?? throw new InvalidOperationException($"Missing monitoring migration V{version:D3}.");

        using var stream = assembly.GetManifestResourceStream(resourceName)!;
        using var reader = new StreamReader(stream);
        return reader.ReadToEnd();
    }

    private static async Task ExecuteAsync(
        NpgsqlConnection connection,
        string sql,
        CancellationToken cancellationToken)
    {
        await using var command = new NpgsqlCommand(sql, connection);
        await command.ExecuteNonQueryAsync(cancellationToken);
    }
}
