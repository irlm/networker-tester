using System.Security.Cryptography;
using System.Text;
using Networker.Monitoring.Data.Migrations;

namespace Networker.Monitoring.Tests;

/// <summary>
/// Analog of the control plane's MigrationScriptFreezeTests for the
/// independent monitoring ledger: every embedded V*.sql in
/// Networker.Monitoring.Data is pinned by SHA-256. Needs no Docker.
/// </summary>
public sealed class MonitoringMigrationFreezeTests
{
    private static readonly IReadOnlyDictionary<string, string> FrozenSha256 = new Dictionary<string, string>
    {
        ["V001_InitialMonitoringSchema.sql"] = "0c1a4057b301372c65df8f8313f3341d597dcc2ed62d68003c37877d2b067130",
    };

    [Fact]
    public void Every_version_up_to_latest_has_an_embedded_script()
    {
        var scripted = ScriptResourceNames()
            .Select(name => int.Parse(FileName(name)[1..4]))
            .ToHashSet();

        Assert.Equal(
            Enumerable.Range(1, MonitoringSchemaMigrator.LatestVersion).ToHashSet(),
            scripted);
    }

    [Fact]
    public void Shipped_monitoring_migrations_are_frozen()
    {
        var resources = ScriptResourceNames();
        Assert.NotEmpty(resources);

        foreach (var resource in resources)
        {
            var fileName = FileName(resource);
            if (!FrozenSha256.TryGetValue(fileName, out var expected))
            {
                // A brand-new migration: allowed, but must be pinned here in
                // the same PR so it freezes from day one.
                Assert.Fail($"New monitoring migration script '{fileName}' has no frozen checksum. " +
                            "Add its SHA-256 to MonitoringMigrationFreezeTests.FrozenSha256.");
            }

            var version = int.Parse(fileName[1..4]);
            var bytes = Encoding.UTF8.GetBytes(MonitoringSchemaMigrator.GetScript(version));
            var actual = Convert.ToHexStringLower(SHA256.HashData(bytes));
            Assert.True(expected == actual,
                $"Monitoring migration script '{fileName}' changed after shipping (sha256 {actual}, pinned {expected}). " +
                "Shipped monitoring migrations are immutable — add a new V00N script instead.");
        }
    }

    private static string[] ScriptResourceNames() =>
        typeof(MonitoringSchemaMigrator).Assembly.GetManifestResourceNames()
            .Where(name => name.Contains(".Migrations.V", StringComparison.Ordinal)
                && name.EndsWith(".sql", StringComparison.Ordinal))
            .ToArray();

    private static string FileName(string resource) =>
        resource[(resource.IndexOf(".Migrations.", StringComparison.Ordinal) + ".Migrations.".Length)..];
}
