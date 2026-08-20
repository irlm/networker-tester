using System.Text.Json;
using System.Text.Json.Nodes;
using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// #782 P1 — URL sets. The endpoint block is stored as opaque JSONB, so a set
/// config's `hosts` array is whatever the caller sent until something
/// canonicalizes it. These pin that canonicalization: what gets repaired, what
/// is rejected, and — most importantly — what is left byte-identical, because
/// every pre-set config in the database must round-trip through create/read
/// unchanged.
/// </summary>
public class TestConfigEndpointNormalizerTests
{
    private static JsonElement Json(string raw) => JsonDocument.Parse(raw).RootElement.Clone();

    private static string Normalize(string raw)
    {
        Assert.True(TestConfigEndpointNormalizer.TryNormalize(Json(raw), out var json, out var error), error);
        Assert.Null(error);
        return json;
    }

    private static string? Reject(string raw)
    {
        var ok = TestConfigEndpointNormalizer.TryNormalize(Json(raw), out _, out var error);
        Assert.False(ok);
        Assert.NotNull(error);
        return error;
    }

    private static List<string> Hosts(string json) =>
        [.. JsonNode.Parse(json)!["hosts"]!.AsArray().Select(n => n!.GetValue<string>())];

    // ── Untouched shapes ──────────────────────────────────────────────────

    [Fact]
    public void Classic_single_host_network_endpoint_is_byte_identical()
    {
        // No `hosts` key at all — the shape every pre-#782 config carries.
        const string raw = """{"kind":"network","host":"https://example.com/","port":8443}""";
        Assert.Equal(raw, Normalize(raw));
    }

    [Theory]
    [InlineData("""{"kind":"proxy","proxy_endpoint_id":"11111111-1111-1111-1111-111111111111"}""")]
    [InlineData("""{"kind":"runtime","runtime_id":"go","language":"go"}""")]
    [InlineData("""{"kind":"pending","cloud_account_id":"a","region":"eastus","vm_size":"Standard_B2s","os":"linux"}""")]
    public void Non_network_endpoints_pass_through(string raw)
    {
        Assert.Equal(raw, Normalize(raw));
    }

    [Fact]
    public void Already_canonical_set_is_byte_identical()
    {
        // host == hosts[0], no blanks, no duplicates: nothing to rewrite, so
        // find_or_create must not see a spurious endpoint change on every
        // re-probe of the same set.
        const string raw =
            """{"kind":"network","host":"https://a.example/","hosts":["https://a.example/","https://b.example/"]}""";
        Assert.Equal(raw, Normalize(raw));
    }

    // ── Repairs ───────────────────────────────────────────────────────────

    [Fact]
    public void Duplicate_members_collapse_to_one()
    {
        // The same URL twice is one probe, but two --target flags: the run
        // would probe it twice while the name and per-URL grouping claim two
        // distinct targets.
        var json = Normalize(
            """{"kind":"network","host":"https://a.example/","hosts":["https://a.example/","https://b.example/","https://a.example/"]}""");
        Assert.Equal(["https://a.example/", "https://b.example/"], Hosts(json));
    }

    [Fact]
    public void Blank_and_whitespace_members_are_dropped_and_survivors_trimmed()
    {
        var json = Normalize(
            """{"kind":"network","host":"https://a.example/","hosts":["https://a.example/","   ","  https://b.example/  ",""]}""");
        Assert.Equal(["https://a.example/", "https://b.example/"], Hosts(json));
    }

    [Fact]
    public void Host_missing_from_hosts_is_prepended_never_dropped()
    {
        // The caller asked for three targets; two of them are in `hosts` and
        // one only in `host`. Discarding the odd one out would silently probe
        // less than was requested.
        var json = Normalize(
            """{"kind":"network","host":"https://a.example/","hosts":["https://b.example/","https://c.example/"]}""");
        Assert.Equal(["https://a.example/", "https://b.example/", "https://c.example/"], Hosts(json));
        Assert.Equal("https://a.example/", JsonNode.Parse(json)!["host"]!.GetValue<string>());
    }

    [Fact]
    public void Host_is_realigned_to_the_first_member()
    {
        // host present but NOT first: the agent's single-target answer
        // (EndpointToTarget → targets[0]) would disagree with `host`.
        var json = Normalize(
            """{"kind":"network","host":"https://b.example/","hosts":["https://a.example/","https://b.example/"]}""");
        Assert.Equal("https://a.example/", JsonNode.Parse(json)!["host"]!.GetValue<string>());
        Assert.Equal(["https://a.example/", "https://b.example/"], Hosts(json));
    }

    [Fact]
    public void Hosts_only_shape_gains_a_host_field()
    {
        var json = Normalize("""{"kind":"network","hosts":["https://a.example/","https://b.example/"]}""");
        Assert.Equal("https://a.example/", JsonNode.Parse(json)!["host"]!.GetValue<string>());
    }

    [Fact]
    public void Unrelated_endpoint_fields_survive_normalization()
    {
        var json = Normalize(
            """{"kind":"network","host":"https://b.example/","port":8443,"hosts":["https://a.example/","https://a.example/"]}""");
        var node = JsonNode.Parse(json)!;
        Assert.Equal(8443, node["port"]!.GetValue<int>());
        Assert.Equal("network", node["kind"]!.GetValue<string>());
    }

    [Fact]
    public void A_single_member_set_stays_a_set()
    {
        // Collapsing `hosts:["x"]` back to the classic shape would make the
        // stored row differ from what the client will send next time, and
        // find_or_create would rewrite the endpoint on every probe.
        var json = Normalize("""{"kind":"network","host":"https://a.example/","hosts":["https://a.example/"]}""");
        Assert.Equal(["https://a.example/"], Hosts(json));
    }

    // ── Rejections ────────────────────────────────────────────────────────

    [Fact]
    public void Empty_or_all_blank_hosts_with_no_host_is_rejected()
    {
        Assert.Contains("at least one non-empty URL", Reject("""{"kind":"network","hosts":[]}"""));
        Assert.Contains("at least one non-empty URL", Reject("""{"kind":"network","hosts":["","  "]}"""));
    }

    [Fact]
    public void Non_string_members_are_rejected()
    {
        Assert.Contains("array of URL strings", Reject("""{"kind":"network","hosts":["https://a.example/",42]}"""));
        Assert.Contains("array of URL strings", Reject("""{"kind":"network","hosts":"https://a.example/"}"""));
    }

    [Fact]
    public void Over_the_member_cap_is_rejected_with_the_count()
    {
        var many = Enumerable.Range(0, TestConfigEndpointNormalizer.MaxSetHosts + 1)
            .Select(i => $"\"https://h{i}.example/\"");
        var error = Reject($$"""{"kind":"network","hosts":[{{string.Join(",", many)}}]}""");
        Assert.Contains($"{TestConfigEndpointNormalizer.MaxSetHosts + 1} URLs", error);
        Assert.Contains($"capped at {TestConfigEndpointNormalizer.MaxSetHosts}", error);
    }

    [Fact]
    public void Exactly_the_member_cap_is_accepted()
    {
        var many = Enumerable.Range(0, TestConfigEndpointNormalizer.MaxSetHosts)
            .Select(i => $"\"https://h{i}.example/\"");
        var json = Normalize($$"""{"kind":"network","hosts":[{{string.Join(",", many)}}]}""");
        Assert.Equal(TestConfigEndpointNormalizer.MaxSetHosts, Hosts(json).Count);
    }

    [Fact]
    public void Duplicates_are_collapsed_before_the_cap_is_applied()
    {
        // 30 lines but only 2 distinct URLs is a 2-URL set, not a rejection.
        var repeated = Enumerable.Range(0, 30).Select(i => i % 2 == 0
            ? "\"https://a.example/\""
            : "\"https://b.example/\"");
        var json = Normalize($$"""{"kind":"network","hosts":[{{string.Join(",", repeated)}}]}""");
        Assert.Equal(["https://a.example/", "https://b.example/"], Hosts(json));
    }
}
