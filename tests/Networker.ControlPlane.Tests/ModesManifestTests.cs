using System.Reflection;
using System.Text.Json;
using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Drift guard: <c>shared/modes.json</c> ⇄ <c>PlatformEndpoints</c> mode catalog.
///
/// The engine (crates/networker-tester/src/metrics.rs, Protocol::all_modes())
/// is the source of truth; <c>shared/modes.json</c> is its canonical
/// machine-readable manifest (guarded on the Rust side by
/// modes_manifest_guard.rs and on the dashboard side by
/// modes-manifest.test.ts). This class guards the C# hand-copy in
/// <c>PlatformEndpoints.AllModes</c> + <c>GroupDetail</c> — the 6-way
/// unguarded copy of this list already shipped bugs (#377-379).
///
/// PlatformEndpoints keeps its catalog private by design (it is an endpoint
/// implementation detail), so the guard reads it via reflection rather than
/// widening the API surface for a test.
/// </summary>
public class ModesManifestTests
{
    private static readonly JsonDocument Manifest = JsonDocument.Parse(
        File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "shared", "modes.json")));

    private sealed record CatalogEntry(
        string Id, string Name, string Description, string Detail, string Group, string Requires);

    /// <summary>Manifest entries that belong in the /api/modes catalog (catalog == true), in order.</summary>
    private static List<CatalogEntry> ManifestCatalog()
    {
        var list = new List<CatalogEntry>();
        foreach (var m in Manifest.RootElement.GetProperty("modes").EnumerateArray())
        {
            if (!m.GetProperty("catalog").GetBoolean())
            {
                continue;
            }

            list.Add(new CatalogEntry(
                m.GetProperty("id").GetString()!,
                m.GetProperty("name").GetString()!,
                m.GetProperty("description").GetString()!,
                m.GetProperty("detail").GetString()!,
                m.GetProperty("group").GetString()!,
                m.GetProperty("requires").GetString()!));
        }

        return list;
    }

    /// <summary>PlatformEndpoints.AllModes via reflection (private static readonly ModeInfo[]).</summary>
    private static List<CatalogEntry> PlatformCatalog()
    {
        var field = typeof(PlatformEndpoints).GetField("AllModes", BindingFlags.NonPublic | BindingFlags.Static);
        Assert.NotNull(field);
        var array = (Array)field!.GetValue(null)!;

        var list = new List<CatalogEntry>();
        foreach (var item in array)
        {
            var t = item.GetType();
            string Prop(string name) => (string)t.GetProperty(name)!.GetValue(item)!;
            list.Add(new CatalogEntry(
                Prop("Id"), Prop("Name"), Prop("Description"), Prop("Detail"), Prop("Group"), Prop("Requires")));
        }

        return list;
    }

    private static string InvokeGroupDetail(string label)
    {
        var method = typeof(PlatformEndpoints).GetMethod("GroupDetail", BindingFlags.NonPublic | BindingFlags.Static);
        Assert.NotNull(method);
        return (string)method!.Invoke(null, [label])!;
    }

    [Fact]
    public void Catalog_ids_match_manifest_in_order()
    {
        var manifest = ManifestCatalog().Select(e => e.Id).ToArray();
        var platform = PlatformCatalog().Select(e => e.Id).ToArray();
        Assert.Equal(manifest, platform);
    }

    [Fact]
    public void Catalog_text_matches_manifest_byte_for_byte()
    {
        var manifest = ManifestCatalog();
        var platform = PlatformCatalog();
        Assert.Equal(manifest.Count, platform.Count);
        foreach (var (m, p) in manifest.Zip(platform))
        {
            Assert.Equal(m, p); // record equality: id + name + description + detail + group
        }
    }

    [Fact]
    public void Group_details_match_manifest()
    {
        foreach (var g in Manifest.RootElement.GetProperty("groups").EnumerateArray())
        {
            var label = g.GetProperty("label").GetString()!;
            var detail = g.GetProperty("detail").GetString()!;
            Assert.Equal(detail, InvokeGroupDetail(label));
        }
    }

    [Fact]
    public void Every_catalog_group_has_a_manifest_group_detail()
    {
        var declared = Manifest.RootElement.GetProperty("groups").EnumerateArray()
            .Select(g => g.GetProperty("label").GetString()!)
            .ToHashSet();
        foreach (var entry in PlatformCatalog())
        {
            Assert.Contains(entry.Group, declared);
        }
    }

    [Fact]
    public void Apibench_is_runner_level_and_in_catalog()
    {
        var apibench = Manifest.RootElement.GetProperty("modes").EnumerateArray()
            .Single(m => m.GetProperty("id").GetString() == "apibench");
        Assert.Equal("runner", apibench.GetProperty("level").GetString());
        Assert.True(apibench.GetProperty("catalog").GetBoolean());
        // And the platform catalog serves it (the wizard needs it in /api/modes).
        Assert.Contains("apibench", PlatformCatalog().Select(e => e.Id));
    }
}
