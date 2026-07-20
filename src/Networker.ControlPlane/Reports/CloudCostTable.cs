using System.Reflection;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Networker.ControlPlane.Reports;

/// <summary>
/// The static, hand-curated cloud price table backing the
/// performance-per-cost report (<c>shared/cloud-costs.json</c>, embedded into
/// this assembly at build time — see the csproj). On-demand hourly LIST
/// prices for the VM sizes the provisioning wizard offers; never fetched from
/// a pricing API at runtime by design (prices change slowly, the report must
/// be reproducible and offline-safe, and every number carries its
/// <c>source_url</c> + <c>as_of</c> date so staleness is visible, not
/// silent). Maintenance procedure: <c>docs/reports-perf-per-cost.md</c>.
/// </summary>
public sealed class CloudCostTable
{
    /// <summary>Cloud ids that may appear in the table — must match the
    /// <c>project_tester.cloud</c> values the provisioner writes.</summary>
    private static readonly string[] KnownProviders = ["azure", "aws", "gcp"];

    // Sanity ceiling for a single VM hour; a curated row above this is a typo
    // (the largest size we provision is ~$0.40/hr).
    private const decimal MaxPlausibleHourlyUsd = 100m;

    private readonly List<CloudCostRate> _rates;

    public string Disclaimer { get; }
    public string AsOf { get; }
    public IReadOnlyList<CloudCostRate> Rates => _rates;

    private CloudCostTable(string disclaimer, string asOf, List<CloudCostRate> rates)
    {
        Disclaimer = disclaimer;
        AsOf = asOf;
        _rates = rates;
    }

    private static readonly Lazy<CloudCostTable> Embedded = new(() =>
    {
        var asm = Assembly.GetExecutingAssembly();
        const string resource = "Networker.ControlPlane.shared.cloud-costs.json";
        using var stream = asm.GetManifestResourceStream(resource)
            ?? throw new InvalidOperationException(
                $"Embedded cost table '{resource}' missing — check the csproj EmbeddedResource entry.");
        using var reader = new StreamReader(stream);
        return Parse(reader.ReadToEnd());
    });

    /// <summary>The validated table embedded from <c>shared/cloud-costs.json</c>.
    /// Throws (once, cached) if the curated file is malformed — a bad price
    /// table should fail loudly at first use, not emit wrong economics.</summary>
    public static CloudCostTable Instance => Embedded.Value;

    /// <summary>Parse + validate a cost-table JSON document. Public so unit
    /// tests can exercise validation without touching the embedded copy.</summary>
    public static CloudCostTable Parse(string json)
    {
        var doc = JsonSerializer.Deserialize<CostTableDocument>(json, JsonOpts)
            ?? throw new InvalidOperationException("cloud-costs: document is null");

        if (string.IsNullOrWhiteSpace(doc.Disclaimer))
        {
            throw new InvalidOperationException("cloud-costs: 'disclaimer' is required");
        }

        if (!DateOnly.TryParse(doc.AsOf, out _))
        {
            throw new InvalidOperationException($"cloud-costs: 'as_of' is not a date: '{doc.AsOf}'");
        }

        if (doc.Rates is null || doc.Rates.Count == 0)
        {
            throw new InvalidOperationException("cloud-costs: 'rates' is empty");
        }

        var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var r in doc.Rates)
        {
            var key = $"{r.Provider}/{r.Sku}/{r.Region}";
            if (!KnownProviders.Contains(r.Provider))
            {
                throw new InvalidOperationException(
                    $"cloud-costs [{key}]: unknown provider '{r.Provider}' (expected azure|aws|gcp)");
            }
            if (string.IsNullOrWhiteSpace(r.Sku) || string.IsNullOrWhiteSpace(r.Region))
            {
                throw new InvalidOperationException($"cloud-costs [{key}]: sku/region must be non-empty");
            }
            if (r.HourlyUsd <= 0 || r.HourlyUsd >= MaxPlausibleHourlyUsd)
            {
                throw new InvalidOperationException(
                    $"cloud-costs [{key}]: hourly_usd {r.HourlyUsd} outside (0, {MaxPlausibleHourlyUsd})");
            }
            if (r.SourceUrl is null || !r.SourceUrl.StartsWith("https://", StringComparison.Ordinal))
            {
                throw new InvalidOperationException($"cloud-costs [{key}]: source_url must be https");
            }
            if (!DateOnly.TryParse(r.AsOf, out _))
            {
                throw new InvalidOperationException($"cloud-costs [{key}]: as_of is not a date: '{r.AsOf}'");
            }
            if (!seen.Add(key))
            {
                throw new InvalidOperationException($"cloud-costs [{key}]: duplicate row");
            }
        }

        return new CloudCostTable(doc.Disclaimer, doc.AsOf, doc.Rates);
    }

    /// <summary>
    /// Resolve the rate for a tester's (provider, sku, region). Exact-region
    /// match wins; when the sku is only priced in another region the report
    /// still uses it but flags <see cref="CostLookup.RegionMatched"/> false so
    /// callers can surface "priced from {row.Region}". Unknown sku → null
    /// (callers must show '—', never silently drop the row).
    /// </summary>
    public CostLookup? Find(string provider, string sku, string region)
    {
        CloudCostRate? fallback = null;
        foreach (var r in _rates)
        {
            if (!r.Provider.Equals(provider, StringComparison.OrdinalIgnoreCase) ||
                !r.Sku.Equals(sku, StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }
            if (r.Region.Equals(region, StringComparison.OrdinalIgnoreCase))
            {
                return new CostLookup(r, RegionMatched: true);
            }
            fallback ??= r;
        }
        return fallback is null ? null : new CostLookup(fallback, RegionMatched: false);
    }

    private static readonly JsonSerializerOptions JsonOpts = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower,
        ReadCommentHandling = JsonCommentHandling.Skip,
    };

    private sealed record CostTableDocument(
        [property: JsonPropertyName("disclaimer")] string? Disclaimer,
        [property: JsonPropertyName("as_of")] string? AsOf,
        [property: JsonPropertyName("rates")] List<CloudCostRate>? Rates);
}

/// <summary>One curated price row from <c>shared/cloud-costs.json</c>.</summary>
public sealed record CloudCostRate(
    [property: JsonPropertyName("provider")] string Provider,
    [property: JsonPropertyName("sku")] string Sku,
    [property: JsonPropertyName("region")] string Region,
    [property: JsonPropertyName("hourly_usd")] decimal HourlyUsd,
    [property: JsonPropertyName("source_url")] string SourceUrl,
    [property: JsonPropertyName("as_of")] string AsOf);

/// <summary>A resolved rate + whether the tester's own region was priced
/// (false = nearest-region fallback; surface it, don't hide it).</summary>
public sealed record CostLookup(CloudCostRate Rate, bool RegionMatched);
