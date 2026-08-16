using System.Reflection;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// The canonical HTTP-stack manifest (<c>shared/http-stacks.json</c>, embedded
/// at build time): the port layout + capabilities of every comparison stack
/// the installers set up on endpoint VMs, and the probe modes that need
/// HTTP/3 on the target (<c>h3_modes</c>). One table for the whole
/// mode ⇄ target seam — the Rust tester embeds the same file
/// (<c>crates/networker-tester/src/http_stacks.rs</c>), the dashboard mirrors
/// it (<c>lib/http-stacks.ts</c>, guarded by <c>http-stacks-manifest.test.ts</c>)
/// and <c>lab/validate.sh</c> reads it — so "apache has no QUIC" is decided in
/// exactly one place.
///
/// <para>Served verbatim by <c>GET /api/http-stacks</c> (and folded into
/// <c>GET /api/modes</c> as <c>stacks</c> / <c>h3_modes</c>) so API clients
/// see the same table the gate enforces. Consumed by
/// <see cref="ModeTargetCompatibility"/> (config-create 422) and the
/// comparison-group launch (per-cell drop).</para>
/// </summary>
public static class HttpStackCatalog
{
    /// <summary>One row of <c>shared/http-stacks.json</c>.</summary>
    public sealed record Stack(
        [property: JsonPropertyName("id")] string Id,
        [property: JsonPropertyName("http_port")] int HttpPort,
        [property: JsonPropertyName("https_port")] int HttpsPort,
        [property: JsonPropertyName("h3")] bool H3,
        [property: JsonPropertyName("installer")] string? Installer);

    /// <summary>The parsed manifest: stacks in file order + the ordered h3-mode list.</summary>
    public sealed record Manifest(
        [property: JsonPropertyName("h3_modes")] IReadOnlyList<string> H3Modes,
        [property: JsonPropertyName("stacks")] IReadOnlyList<Stack> Stacks);

    private static readonly Lazy<Manifest> Embedded = new(() =>
    {
        var asm = Assembly.GetExecutingAssembly();
        const string resource = "Networker.ControlPlane.shared.http-stacks.json";
        using var stream = asm.GetManifestResourceStream(resource)
            ?? throw new InvalidOperationException(
                $"Embedded HTTP-stack manifest '{resource}' missing — check the csproj EmbeddedResource entry.");
        using var reader = new StreamReader(stream);
        return Parse(reader.ReadToEnd());
    });

    private static readonly Lazy<HashSet<string>> H3ModeSet = new(() =>
        Embedded.Value.H3Modes.ToHashSet(StringComparer.OrdinalIgnoreCase));

    /// <summary>Parse + validate a manifest document. Public so tests can
    /// validate the repo copy without touching the embedded one.</summary>
    public static Manifest Parse(string json)
    {
        var doc = JsonSerializer.Deserialize<Manifest>(json)
            ?? throw new InvalidOperationException("http-stacks: document is null");
        if (doc.Stacks is null || doc.Stacks.Count == 0)
        {
            throw new InvalidOperationException("http-stacks: 'stacks' is empty");
        }
        if (doc.H3Modes is null || doc.H3Modes.Count == 0)
        {
            throw new InvalidOperationException("http-stacks: 'h3_modes' is empty");
        }
        var ids = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var s in doc.Stacks)
        {
            if (string.IsNullOrWhiteSpace(s.Id) || !ids.Add(s.Id))
            {
                throw new InvalidOperationException($"http-stacks: blank or duplicate stack id '{s.Id}'");
            }
        }
        return doc;
    }

    /// <summary>The embedded manifest (parsed once; throws loudly if malformed).</summary>
    public static Manifest Instance => Embedded.Value;

    /// <summary>All stacks in manifest order (<c>endpoint</c> = the bare networker-endpoint).</summary>
    public static IReadOnlyList<Stack> Stacks => Embedded.Value.Stacks;

    /// <summary>Probe modes that need HTTP/3 (QUIC) on the target, in manifest order.</summary>
    public static IReadOnlyList<string> H3Modes => Embedded.Value.H3Modes;

    /// <summary>Case-insensitive lookup; null for unknown / blank ids.</summary>
    public static Stack? Find(string? id)
    {
        if (string.IsNullOrWhiteSpace(id))
        {
            return null;
        }
        var wanted = id.Trim();
        foreach (var s in Stacks)
        {
            if (string.Equals(s.Id, wanted, StringComparison.OrdinalIgnoreCase))
            {
                return s;
            }
        }
        return null;
    }

    /// <summary>Whether a stack serves HTTP/3 — <c>null</c> when the stack is
    /// not in the manifest (callers fail open on unknown stacks).</summary>
    public static bool? HasH3(string? stack) => Find(stack)?.H3;

    /// <summary>True when <paramref name="mode"/> needs HTTP/3 on the target.</summary>
    public static bool IsH3Mode(string? mode) =>
        !string.IsNullOrWhiteSpace(mode) && H3ModeSet.Value.Contains(mode.Trim());

    /// <summary>The wire shape for <c>GET /api/http-stacks</c> (also folded
    /// into <c>GET /api/modes</c>): <c>{ h3_modes: [...], stacks: [...] }</c>.</summary>
    public static object ToWire() => new
    {
        h3_modes = H3Modes.ToArray(),
        stacks = Stacks.Select(s => new
        {
            id = s.Id,
            http_port = s.HttpPort,
            https_port = s.HttpsPort,
            h3 = s.H3,
            installer = s.Installer,
        }).ToArray(),
    };
}
