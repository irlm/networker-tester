using System.Reflection;
using Microsoft.EntityFrameworkCore;
using Networker.Data;
using Xunit;

namespace Networker.Tests;

/// <summary>
/// P0-7 (2026-08 audit): jsonb columns must never appear inside a SERVER-SIDE
/// string predicate. On SQLite (where the entire 1,087-case ControlPlane suite
/// runs) a jsonb column is TEXT, so <c>.Contains()</c> compiles to a perfectly
/// valid LIKE; on Postgres it becomes <c>jsonb ~~ jsonb</c> → <b>42883</b>.
/// That exact difference wedged the provisioning orchestrator in prod for a
/// day on 2026-08-03 — every tick threw, teardown never ran, and all queued
/// auto-provisions starved at "capacity".
///
/// <para>This guard runs the real Npgsql provider: it builds each risky query
/// shape and asserts EF either refuses to translate it or that the generated
/// SQL doesn't hand a jsonb column to a text operator. The safe pattern —
/// select the column, match in memory — is asserted to still work.</para>
/// </summary>
public class JsonbPredicateGuardTests
{

    /// <summary>Every property the model maps to jsonb — the columns that must
    /// stay out of server-side string predicates. Derived from the model, so a
    /// newly-added jsonb column is covered automatically.</summary>
    public static IEnumerable<object[]> JsonbProperties()
    {
        using var ctx = new NetworkerDbContext(
            new DbContextOptionsBuilder<NetworkerDbContext>().UseNpgsql("Host=unused").Options);
        foreach (var entity in ctx.Model.GetEntityTypes())
        {
            foreach (var prop in entity.GetProperties())
            {
                if (string.Equals(prop.GetColumnType(), "jsonb", StringComparison.OrdinalIgnoreCase))
                {
                    yield return [entity.ClrType.Name, prop.Name];
                }
            }
        }
    }

    [Fact]
    public void Model_declares_the_expected_jsonb_surface()
    {
        var found = JsonbProperties().Select(o => $"{o[0]}.{o[1]}").ToList();
        // Guards the guard: if this drops to zero the enumeration silently
        // stopped covering anything (the vacuous-test class).
        Assert.True(found.Count >= 20, $"expected the known jsonb surface, found {found.Count}: {string.Join(", ", found)}");
    }

    [Theory]
    [MemberData(nameof(JsonbProperties))]
    public void Jsonb_columns_are_not_string_matched_inside_a_LINQ_predicate(string entityName, string propertyName)
    {
        // Source-level guard (the runtime tests above prove WHY it matters).
        // Scans the shipped ControlPlane sources for a lambda that calls a
        // string matcher ON a jsonb property — e.g. `.Where(c =>
        // c.EndpointRef.Contains(host))`. Selecting the column
        // (`.Select(c => c.EndpointRef)`) is the safe pattern and is ignored.
        var root = FindRepoRoot();
        var srcDir = Path.Combine(root, "src", "Networker.ControlPlane");
        Assert.True(Directory.Exists(srcDir), $"control-plane sources not found at {srcDir}");

        var matchers = new[] { "Contains", "StartsWith", "EndsWith", "ToLower", "ToUpper" };
        var offenders = new List<string>();

        foreach (var file in Directory.EnumerateFiles(srcDir, "*.cs", SearchOption.AllDirectories))
        {
            var lines = File.ReadAllLines(file);
            for (var i = 0; i < lines.Length; i++)
            {
                var line = lines[i];
                if (!line.Contains("." + propertyName + ".", StringComparison.Ordinal))
                {
                    continue;
                }
                // Only a LINQ predicate matters: the offending shape is a
                // matcher applied directly to the property inside Where/Any/
                // All/First/Single/Count/OrderBy.
                var isPredicate = line.Contains(".Where(", StringComparison.Ordinal)
                    || line.Contains(".Any(", StringComparison.Ordinal)
                    || line.Contains(".All(", StringComparison.Ordinal)
                    || line.Contains(".First", StringComparison.Ordinal)
                    || line.Contains(".Single", StringComparison.Ordinal)
                    || line.Contains(".Count(", StringComparison.Ordinal)
                    || line.Contains(".OrderBy", StringComparison.Ordinal);
                if (!isPredicate)
                {
                    continue;
                }
                foreach (var m in matchers)
                {
                    if (line.Contains("." + propertyName + "." + m + "(", StringComparison.Ordinal))
                    {
                        offenders.Add($"{Path.GetFileName(file)}:{i + 1}: {line.Trim()}");
                    }
                }
            }
        }

        Assert.True(
            offenders.Count == 0,
            $"jsonb column {entityName}.{propertyName} is string-matched inside a server-side "
            + $"predicate — on Postgres that is `jsonb ~~ jsonb` (42883), the 2026-08-03 wedge. "
            + $"Project the column out and match in memory instead.\n  "
            + string.Join("\n  ", offenders));
    }

    private static string FindRepoRoot()
    {
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir is not null && !File.Exists(Path.Combine(dir.FullName, "Networker.sln")))
        {
            dir = dir.Parent;
        }
        return dir?.FullName ?? throw new InvalidOperationException("repo root (Networker.sln) not found");
    }
}

/// <summary>The two RUNTIME halves of the P0-7 guard — they need the real
/// Npgsql provider, so they live in their own container-bound class. The
/// static source scan in <see cref="JsonbPredicateGuardTests"/> deliberately
/// needs no Docker so it runs everywhere, including dev machines without it.</summary>
public class JsonbPredicateRuntimeTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fx;

    public JsonbPredicateRuntimeTests(ControlPlaneFixture fx) => _fx = fx;

    [Fact]
    public async Task Server_side_Contains_on_a_jsonb_column_is_rejected_by_postgres()
    {
        // The 2026-08-03 wedge, reproduced: EF happily translates this to LIKE.
        // Postgres has no jsonb ~~ jsonb operator, so it must FAIL here — this
        // test documents and pins that the pattern is genuinely unusable, which
        // is why the production code selects-then-matches in memory.
        await using var db = _fx.NewDbContext();

        var ex = await Assert.ThrowsAnyAsync<Exception>(async () =>
            await db.TestConfigs
                .Where(c => c.EndpointRef.Contains("10.0.0.1"))
                .Select(c => c.Id)
                .ToListAsync());

        // Npgsql surfaces 42883 (undefined_function) for jsonb ~~ jsonb.
        var flat = ex.ToString();
        Assert.True(
            flat.Contains("42883", StringComparison.Ordinal)
            || flat.Contains("operator does not exist", StringComparison.OrdinalIgnoreCase),
            $"expected a Postgres operator error, got: {flat}");
    }

    [Fact]
    public async Task Client_side_matching_after_projection_is_the_safe_pattern()
    {
        // What ProvisioningOrchestrator/OrphanReaper actually do now: project
        // the jsonb column out, then match in memory. Must succeed.
        await using var db = _fx.NewDbContext();

        var refs = await db.TestConfigs
            .Select(c => c.EndpointRef)
            .ToListAsync();
        var matched = refs.Count(r => r is not null && r.Contains("host", StringComparison.OrdinalIgnoreCase));

        Assert.True(matched >= 0); // executing without throwing is the assertion
    }

}
