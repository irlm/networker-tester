using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Guards the per-language capability matrix (audit C5 / P1#14) that
/// <c>GET /api/modes</c> exposes to the Application Benchmark wizard. Each
/// expectation is source-verified against the reference implementation, not
/// the audit snapshot — see BenchmarkLanguageCapabilities doc comments for the
/// file-level evidence.
/// </summary>
public sealed class BenchmarkLanguageCapabilitiesTests
{
    [Fact]
    public void Every_language_supports_http1()
    {
        Assert.All(BenchmarkLanguageCapabilities.All, c => Assert.True(c.Http1));
    }

    [Fact]
    public void Nginx_is_the_only_language_without_apibench()
    {
        var noApi = BenchmarkLanguageCapabilities.All
            .Where(c => !c.Apibench)
            .Select(c => c.Language)
            .ToArray();

        string[] expected = ["nginx"];
        Assert.Equal(expected, noApi);
    }

    [Fact]
    public void Direct_h1_only_languages_match_source_reality()
    {
        // Boost.Beast (cpp), HttpListener (net48), com.sun.net.httpserver
        // (java), uvicorn (python), puma (ruby), Swoole without
        // open_http2_protocol (php) — all HTTP/1.1-only when self-terminating.
        var h1Only = BenchmarkLanguageCapabilities.All
            .Where(c => !c.Http2 && !c.Http3)
            .Select(c => c.Language)
            .OrderBy(l => l, StringComparer.Ordinal)
            .ToArray();

        string[] expected = ["cpp", "csharp-net48", "java", "php", "python", "ruby"];
        Assert.Equal(expected, h1Only);
    }

    [Fact]
    public void Http3_capable_set_matches_source_reality()
    {
        // quinn (rust), quic-go (go), nginx quic listener, Kestrel h3 on
        // .NET 7+ (net7..net10 incl. AOT).
        var h3 = BenchmarkLanguageCapabilities.All
            .Where(c => c.Http3)
            .Select(c => c.Language)
            .OrderBy(l => l, StringComparer.Ordinal)
            .ToArray();

        string[] expected =
        [
            "csharp-net10", "csharp-net10-aot", "csharp-net7", "csharp-net8",
            "csharp-net8-aot", "csharp-net9", "csharp-net9-aot", "go", "nginx", "rust",
        ];
        Assert.Equal(expected, h3);
    }

    [Fact]
    public void No_language_claims_h3_without_h2()
    {
        // ALPN/Alt-Svc reality: every h3-capable stack here also negotiates h2.
        Assert.All(
            BenchmarkLanguageCapabilities.All.Where(c => c.Http3),
            c => Assert.True(c.Http2, $"{c.Language} claims h3 without h2"));
    }

    [Fact]
    public void Covers_every_wizard_catalog_language_plus_detectable_net6_net7()
    {
        // Mirror of dashboard/src/components/wizard/testbed-constants.ts
        // LANGUAGE_GROUPS, plus csharp-net6/net7 which the SSH probe's
        // csharp-net* sweep can still report from older VMs (audit C4).
        string[] wizardIds =
        [
            "rust", "go", "cpp",
            "csharp-net48", "csharp-net8", "csharp-net8-aot", "csharp-net9",
            "csharp-net9-aot", "csharp-net10", "csharp-net10-aot", "java",
            "nodejs", "python", "ruby", "php",
            "nginx",
        ];

        foreach (var id in wizardIds)
        {
            Assert.NotNull(BenchmarkLanguageCapabilities.Find(id));
        }

        Assert.NotNull(BenchmarkLanguageCapabilities.Find("csharp-net6"));
        Assert.NotNull(BenchmarkLanguageCapabilities.Find("csharp-net7"));
        Assert.Null(BenchmarkLanguageCapabilities.Find("cobol"));
    }

    [Fact]
    public void Language_ids_are_unique()
    {
        var ids = BenchmarkLanguageCapabilities.All.Select(c => c.Language).ToArray();
        Assert.Equal(ids.Length, ids.Distinct(StringComparer.Ordinal).Count());
    }
}
