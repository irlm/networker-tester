using System.Reflection;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// The canonical LagHound SDK <b>sample</b> catalog (<c>shared/sdk-samples.json</c>,
/// embedded at build time): one row per reference app under
/// <c>sdk/&lt;lang&gt;/example</c> — the port LagHound deploys it on, the
/// Dockerfile that builds it, and the SDK package version it reports as
/// <c>sdk.version</c> on <c>GET {prefix}/health</c> (contract v1,
/// <c>shared/sdk-contract-v1.json</c>).
///
/// <para>Same manifest topology as <see cref="HttpStackCatalog"/>: the file
/// lives in <c>shared/</c>, the control plane embeds it, and
/// <c>SdkSampleManifestTests</c> re-derives every <c>sdk_version</c> from the
/// language's real package manifest so the catalog can never drift from the
/// source it describes. <c>install.sh --setup-sdk-samples</c> reads the same
/// ports through the deploy config's <c>endpoints[].sdk_samples</c>.</para>
///
/// <para><b>Why a version field at all:</b> "the sample is newer than what is
/// deployed" needs two numbers. The deployed one is live (the running sample
/// self-reports it); the current one is this catalog, which is the SDK package
/// version for that language — NOT the repo release version, because each SDK
/// versions independently of the five-file release sync (see
/// <c>sdk/rust/Cargo.toml</c>'s workspace note).</para>
/// </summary>
public static class SdkSampleCatalog
{
    /// <summary>One row of <c>shared/sdk-samples.json</c>.</summary>
    public sealed record Sample(
        [property: JsonPropertyName("id")] string Id,
        [property: JsonPropertyName("language")] string Language,
        [property: JsonPropertyName("runtime")] string Runtime,
        [property: JsonPropertyName("description")] string Description,
        [property: JsonPropertyName("sdk_version")] string SdkVersion,
        [property: JsonPropertyName("version_source")] string VersionSource,
        [property: JsonPropertyName("port")] int Port,
        [property: JsonPropertyName("dockerfile")] string Dockerfile,
        [property: JsonPropertyName("source_dir")] string SourceDir);

    /// <summary>The parsed manifest: samples in file order + the route defaults.</summary>
    public sealed record Manifest(
        [property: JsonPropertyName("prefix_default")] string PrefixDefault,
        [property: JsonPropertyName("route_default")] string RouteDefault,
        [property: JsonPropertyName("samples")] IReadOnlyList<Sample> Samples);

    private static readonly Lazy<Manifest> Embedded = new(() =>
    {
        var asm = Assembly.GetExecutingAssembly();
        const string resource = "Networker.ControlPlane.shared.sdk-samples.json";
        using var stream = asm.GetManifestResourceStream(resource)
            ?? throw new InvalidOperationException(
                $"Embedded SDK-sample manifest '{resource}' missing — check the csproj EmbeddedResource entry.");
        using var reader = new StreamReader(stream);
        return Parse(reader.ReadToEnd());
    });

    /// <summary>Parse + validate a manifest document. Public so tests can
    /// validate the repo copy without touching the embedded one.</summary>
    public static Manifest Parse(string json)
    {
        var doc = JsonSerializer.Deserialize<Manifest>(json)
            ?? throw new InvalidOperationException("sdk-samples: document is null");
        if (doc.Samples is null || doc.Samples.Count == 0)
        {
            throw new InvalidOperationException("sdk-samples: 'samples' is empty");
        }
        if (string.IsNullOrWhiteSpace(doc.PrefixDefault) || !doc.PrefixDefault.StartsWith('/'))
        {
            throw new InvalidOperationException("sdk-samples: 'prefix_default' must be an absolute path");
        }
        if (string.IsNullOrWhiteSpace(doc.RouteDefault) || !doc.RouteDefault.StartsWith('/'))
        {
            throw new InvalidOperationException("sdk-samples: 'route_default' must be an absolute path");
        }
        var ids = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var ports = new HashSet<int>();
        foreach (var s in doc.Samples)
        {
            if (string.IsNullOrWhiteSpace(s.Id) || !ids.Add(s.Id))
            {
                throw new InvalidOperationException($"sdk-samples: blank or duplicate sample id '{s.Id}'");
            }
            if (s.Port is < 1 or > 65535 || !ports.Add(s.Port))
            {
                throw new InvalidOperationException($"sdk-samples: sample '{s.Id}' has a duplicate or out-of-range port {s.Port}");
            }
            if (string.IsNullOrWhiteSpace(s.SdkVersion))
            {
                throw new InvalidOperationException($"sdk-samples: sample '{s.Id}' has no sdk_version");
            }
        }
        return doc;
    }

    /// <summary>The embedded manifest (parsed once; throws loudly if malformed).</summary>
    public static Manifest Instance => Embedded.Value;

    /// <summary>All samples in manifest order.</summary>
    public static IReadOnlyList<Sample> Samples => Embedded.Value.Samples;

    /// <summary>The default probe route an SDK endpoint registration uses.</summary>
    public static string RouteDefault => Embedded.Value.RouteDefault;

    /// <summary>The default contract prefix (<c>/laghound</c>) health/info hang off.</summary>
    public static string PrefixDefault => Embedded.Value.PrefixDefault;

    /// <summary>Case-insensitive lookup; null for unknown / blank ids.</summary>
    public static Sample? Find(string? id)
    {
        if (string.IsNullOrWhiteSpace(id))
        {
            return null;
        }
        var wanted = id.Trim();
        foreach (var s in Samples)
        {
            if (string.Equals(s.Id, wanted, StringComparison.OrdinalIgnoreCase))
            {
                return s;
            }
        }
        return null;
    }

    /// <summary>
    /// Normalise a caller-supplied language list to catalog ids, in catalog
    /// order, deduplicated. <paramref name="unknown"/> collects anything that is
    /// not a catalog id so the caller can 400 instead of silently dropping it.
    /// </summary>
    public static List<string> NormalizeLanguages(IEnumerable<string>? requested, out List<string> unknown)
    {
        unknown = [];
        var wanted = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var raw in requested ?? [])
        {
            if (string.IsNullOrWhiteSpace(raw))
            {
                continue;
            }
            var sample = Find(raw);
            if (sample is null)
            {
                unknown.Add(raw.Trim());
                continue;
            }
            wanted.Add(sample.Id);
        }
        return Samples.Where(s => wanted.Contains(s.Id)).Select(s => s.Id).ToList();
    }

    /// <summary>The wire shape for the catalog half of
    /// <c>GET /api/projects/{id}/sdk-endpoints/samples</c>.</summary>
    public static object ToWire() => new
    {
        prefix_default = PrefixDefault,
        route_default = RouteDefault,
        samples = Samples.Select(s => new
        {
            id = s.Id,
            language = s.Language,
            runtime = s.Runtime,
            description = s.Description,
            sdk_version = s.SdkVersion,
            port = s.Port,
            dockerfile = s.Dockerfile,
            source_dir = s.SourceDir,
        }).ToArray(),
    };
}
