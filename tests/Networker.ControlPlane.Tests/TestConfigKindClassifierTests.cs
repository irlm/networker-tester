using System.Text.Json;
using Networker.ControlPlane.Endpoints;
using Networker.Data;

namespace Networker.ControlPlane.Tests;

public sealed class TestConfigKindClassifierTests
{
    [Theory]
    [InlineData("network")]
    [InlineData("url_probe")]
    [InlineData("sdk_probe")]
    [InlineData("benchmark")]
    public void Explicit_kind_is_normalized_and_preserved(string expected)
    {
        using var workload = JsonDocument.Parse("""{"modes":["tcp"]}""");

        var valid = TestConfigKindClassifier.TryResolve(
            expected.ToUpperInvariant(), workload.RootElement, null, out var actual);

        Assert.True(valid);
        Assert.Equal(expected, actual);
    }

    [Fact]
    public void Invalid_explicit_kind_is_rejected()
    {
        using var workload = JsonDocument.Parse("""{"modes":["tcp"]}""");

        Assert.False(TestConfigKindClassifier.TryResolve(
            "synthetic", workload.RootElement, null, out _));
    }

    [Theory]
    [InlineData("sdkprobe", TestConfigKinds.SdkProbe)]
    [InlineData("apibench", TestConfigKinds.Benchmark)]
    [InlineData("tcp", TestConfigKinds.Network)]
    public void Legacy_clients_are_classified_from_workload(string mode, string expected)
    {
        using var workload = JsonDocument.Parse($$"""{"modes":["{{mode}}"]}""");

        Assert.True(TestConfigKindClassifier.TryResolve(
            null, workload.RootElement, null, out var actual));
        Assert.Equal(expected, actual);
    }

    [Fact]
    public void Methodology_classifies_a_legacy_config_as_benchmark()
    {
        using var workload = JsonDocument.Parse("""{"modes":["http2"]}""");
        using var methodology = JsonDocument.Parse("""{}""");

        Assert.True(TestConfigKindClassifier.TryResolve(
            null, workload.RootElement, methodology.RootElement, out var actual));
        Assert.Equal(TestConfigKinds.Benchmark, actual);
    }
}

public sealed class TestConfigKindClassifierNameRuleTests
{
    private static System.Text.Json.JsonElement Workload(string json) =>
        System.Text.Json.JsonDocument.Parse(json).RootElement.Clone();

    [Theory]
    [InlineData("Diag: api.example.com (Quick)")]
    [InlineData("Diag set: api.example.com +2 (Quick)")]
    [InlineData("Probe: old.example.com (Full)")]
    public void Legacy_writers_with_diag_style_names_classify_as_url_probe(string name)
    {
        // Mirror of the V053 backfill rule: a rollout-window client that sends
        // no explicit test_kind must not permanently pin a probe as 'network'.
        Assert.True(Networker.ControlPlane.Endpoints.TestConfigKindClassifier.TryResolve(
            null, Workload("""{"modes":["http2"]}"""), null, out var kind, name));
        Assert.Equal(Networker.Data.TestConfigKinds.UrlProbe, kind);
    }

    [Fact]
    public void Explicit_kind_still_wins_over_the_name_rule()
    {
        Assert.True(Networker.ControlPlane.Endpoints.TestConfigKindClassifier.TryResolve(
            "network", Workload("""{"modes":["http2"]}"""), null, out var kind, "Diag: x"));
        Assert.Equal(Networker.Data.TestConfigKinds.Network, kind);
    }
}
