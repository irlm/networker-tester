using Networker.ControlPlane.Reports;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Validation + lookup tests for the static curated price table. The embedded
/// copy of <c>shared/cloud-costs.json</c> is itself parsed here, so a curation
/// mistake (typo'd price, http source, duplicate row, bad date) fails CI —
/// not the first prod request for the report.
/// </summary>
public sealed class CloudCostTableTests
{
    private static string ValidJson(string ratesJson) => $$"""
        {
          "disclaimer": "test disclaimer",
          "as_of": "2026-07-20",
          "rates": [{{ratesJson}}]
        }
        """;

    private const string AzureB1s = """
        { "provider": "azure", "sku": "Standard_B1s", "region": "eastus",
          "hourly_usd": 0.0104, "source_url": "https://prices.azure.com/x", "as_of": "2026-07-20" }
        """;

    // ── the embedded curated file itself ────────────────────────────────────

    [Fact]
    public void Embedded_table_loads_and_validates()
    {
        var table = CloudCostTable.Instance;

        Assert.NotEmpty(table.Disclaimer);
        Assert.NotEmpty(table.AsOf);
        // One row per provisioning-wizard VM size (testbed-constants.ts):
        // 6 Azure + 6 AWS + 6 GCP.
        Assert.Equal(18, table.Rates.Count);
        Assert.Equal(
            new[] { "aws", "azure", "gcp" },
            table.Rates.Select(r => r.Provider).Distinct().Order().ToArray());
    }

    [Fact]
    public void Embedded_table_prices_every_wizard_vm_size_in_its_primary_region()
    {
        var table = CloudCostTable.Instance;

        foreach (var (provider, region, sku) in new (string, string, string)[]
        {
            ("azure", "eastus", "Standard_B1s"), ("azure", "eastus", "Standard_B2s"),
            ("azure", "eastus", "Standard_B2ms"), ("azure", "eastus", "Standard_D2s_v3"),
            ("azure", "eastus", "Standard_D4s_v5"), ("azure", "eastus", "Standard_D8s_v5"),
            ("aws", "us-east-1", "t3.micro"), ("aws", "us-east-1", "t3.small"),
            ("aws", "us-east-1", "t3.medium"), ("aws", "us-east-1", "t3.large"),
            ("aws", "us-east-1", "m5.large"), ("aws", "us-east-1", "m5.xlarge"),
            ("gcp", "us-east1", "e2-micro"), ("gcp", "us-east1", "e2-small"),
            ("gcp", "us-east1", "e2-medium"), ("gcp", "us-east1", "e2-standard-2"),
            ("gcp", "us-east1", "e2-standard-4"), ("gcp", "us-east1", "e2-standard-8"),
        })
        {
            var hit = table.Find(provider, sku, region);
            Assert.NotNull(hit);
            Assert.True(hit!.RegionMatched, $"{provider}/{sku}/{region} not priced in-region");
        }
    }

    // ── parsing + validation ────────────────────────────────────────────────

    [Fact]
    public void Parse_rejects_missing_disclaimer()
    {
        var ex = Assert.Throws<InvalidOperationException>(() => CloudCostTable.Parse(
            """{ "disclaimer": "", "as_of": "2026-07-20", "rates": [] }"""));
        Assert.Contains("disclaimer", ex.Message);
    }

    [Fact]
    public void Parse_rejects_unparseable_as_of_date()
    {
        var ex = Assert.Throws<InvalidOperationException>(() => CloudCostTable.Parse(
            """{ "disclaimer": "d", "as_of": "not-a-date", "rates": [] }"""));
        Assert.Contains("as_of", ex.Message);
    }

    [Fact]
    public void Parse_rejects_empty_rates()
    {
        var ex = Assert.Throws<InvalidOperationException>(() => CloudCostTable.Parse(
            """{ "disclaimer": "d", "as_of": "2026-07-20", "rates": [] }"""));
        Assert.Contains("empty", ex.Message);
    }

    [Theory]
    [InlineData("oracle", "unknown provider")] // not a cloud we provision
    [InlineData("Azure ", "unknown provider")] // exact ids only, no trailing junk
    public void Parse_rejects_unknown_provider(string provider, string expected)
    {
        var row = AzureB1s.Replace("\"azure\"", $"\"{provider}\"");
        var ex = Assert.Throws<InvalidOperationException>(
            () => CloudCostTable.Parse(ValidJson(row)));
        Assert.Contains(expected, ex.Message);
    }

    [Theory]
    [InlineData("0")]     // free VMs are a typo
    [InlineData("-0.01")] // negative is corrupt
    [InlineData("150")]   // above the plausibility ceiling
    public void Parse_rejects_implausible_hourly_usd(string price)
    {
        var row = AzureB1s.Replace("0.0104", price);
        var ex = Assert.Throws<InvalidOperationException>(
            () => CloudCostTable.Parse(ValidJson(row)));
        Assert.Contains("hourly_usd", ex.Message);
    }

    [Fact]
    public void Parse_rejects_non_https_source_url()
    {
        var row = AzureB1s.Replace("https://", "http://");
        var ex = Assert.Throws<InvalidOperationException>(
            () => CloudCostTable.Parse(ValidJson(row)));
        Assert.Contains("source_url", ex.Message);
    }

    [Fact]
    public void Parse_rejects_duplicate_rows()
    {
        var ex = Assert.Throws<InvalidOperationException>(
            () => CloudCostTable.Parse(ValidJson($"{AzureB1s}, {AzureB1s}")));
        Assert.Contains("duplicate", ex.Message);
    }

    // ── lookup semantics ────────────────────────────────────────────────────

    [Fact]
    public void Find_prefers_exact_region_match()
    {
        var westRow = AzureB1s.Replace("eastus", "westeurope").Replace("0.0104", "0.0118");
        var table = CloudCostTable.Parse(ValidJson($"{AzureB1s}, {westRow}"));

        var hit = table.Find("azure", "Standard_B1s", "westeurope");

        Assert.NotNull(hit);
        Assert.True(hit!.RegionMatched);
        Assert.Equal(0.0118m, hit.Rate.HourlyUsd);
    }

    [Fact]
    public void Find_falls_back_to_another_region_and_flags_it()
    {
        var table = CloudCostTable.Parse(ValidJson(AzureB1s));

        var hit = table.Find("azure", "Standard_B1s", "westeurope");

        Assert.NotNull(hit);
        Assert.False(hit!.RegionMatched); // caller must surface "priced from eastus"
        Assert.Equal("eastus", hit.Rate.Region);
    }

    [Fact]
    public void Find_returns_null_for_unknown_sku_so_rows_show_a_dash_not_a_guess()
    {
        var table = CloudCostTable.Parse(ValidJson(AzureB1s));

        Assert.Null(table.Find("azure", "Standard_NC96ads_A100_v4", "eastus"));
        Assert.Null(table.Find("aws", "Standard_B1s", "eastus")); // sku is per-provider
    }

    [Fact]
    public void Find_is_case_insensitive_on_ids()
    {
        var table = CloudCostTable.Parse(ValidJson(AzureB1s));

        var hit = table.Find("Azure", "standard_b1s", "EASTUS");

        Assert.NotNull(hit);
        Assert.True(hit!.RegionMatched);
    }
}
