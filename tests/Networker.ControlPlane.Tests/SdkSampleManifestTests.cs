using System.Text.Json;
using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Drift guard for <c>shared/sdk-samples.json</c>.
///
/// <para>The catalog's <c>sdk_version</c> is the ONLY definition of "what the
/// sample is now" — the SDK Endpoints page compares it against the version a
/// deployed sample self-reports to decide whether to offer an update. A catalog
/// that drifts from the SDK sources would either hide a real update or offer a
/// phantom one, so every row is re-derived here from the language's real
/// package manifest.</para>
///
/// <para>These tests read the repo tree (not a copied resource): the sources
/// they check — <c>sdk/js/package.json</c>, <c>sdk/rust/Cargo.toml</c>, … —
/// only exist there.</para>
/// </summary>
public class SdkSampleManifestTests
{
    private static string RepoRoot()
    {
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir is not null && !Directory.Exists(Path.Combine(dir.FullName, ".github")))
        {
            dir = dir.Parent;
        }
        Assert.True(dir is not null, "could not locate the repository root from the test binary");
        return dir!.FullName;
    }

    private static string Read(params string[] parts) =>
        File.ReadAllText(Path.Combine(RepoRoot(), Path.Combine(parts)));

    [Fact]
    public void Embedded_copy_matches_the_repo_file()
    {
        var repo = SdkSampleCatalog.Parse(Read("shared", "sdk-samples.json"));
        Assert.Equal(repo.Samples, SdkSampleCatalog.Samples);
        Assert.Equal(repo.RouteDefault, SdkSampleCatalog.RouteDefault);
        Assert.Equal(repo.PrefixDefault, SdkSampleCatalog.PrefixDefault);
    }

    [Fact]
    public void Catalog_covers_every_sample_the_repo_actually_ships()
    {
        // examples/<id>.Dockerfile is what builds each sample; a new sample
        // added there without a catalog row would be invisible to the page.
        var dockerfiles = Directory
            .GetFiles(Path.Combine(RepoRoot(), "examples"), "*.Dockerfile")
            .Select(f => Path.GetFileNameWithoutExtension(f))
            .OrderBy(x => x, StringComparer.Ordinal)
            .ToList();
        var ids = SdkSampleCatalog.Samples.Select(s => s.Id).OrderBy(x => x, StringComparer.Ordinal).ToList();
        Assert.Equal(dockerfiles, ids);

        foreach (var s in SdkSampleCatalog.Samples)
        {
            Assert.True(File.Exists(Path.Combine(RepoRoot(), s.Dockerfile)),
                $"{s.Id}: dockerfile '{s.Dockerfile}' does not exist");
            Assert.True(Directory.Exists(Path.Combine(RepoRoot(), s.SourceDir)),
                $"{s.Id}: source_dir '{s.SourceDir}' does not exist");
        }
    }

    [Fact]
    public void Every_sdk_version_is_re_derived_from_the_language_source()
    {
        Assert.Equal(JsPackageVersion(), Version("js"));
        Assert.Equal(PythonVersion(), Version("python"));
        Assert.Equal(RustCrateVersion(), Version("rust"));
        Assert.Equal(GoConstVersion(), Version("go"));
        Assert.Equal(CsharpProjectVersion(), Version("csharp"));
    }

    [Fact]
    public void Sample_ports_never_collide_with_anything_else_on_an_endpoint_host()
    {
        // install.sh puts the samples on the SAME VM as networker-endpoint and
        // (optionally) a proxy stack. A collision would look like a healthy
        // deploy whose sample silently failed to bind.
        var taken = new HashSet<int> { 8080, 8443, 8085 };
        foreach (var stack in HttpStackCatalog.Stacks)
        {
            taken.Add(stack.HttpPort);
            taken.Add(stack.HttpsPort);
        }
        foreach (var s in SdkSampleCatalog.Samples)
        {
            Assert.False(taken.Contains(s.Port),
                $"sample '{s.Id}' port {s.Port} collides with an endpoint/proxy port already used on a deployed host");
        }
    }

    [Fact]
    public void Installer_port_table_matches_the_manifest()
    {
        // install.sh cannot read the manifest (bash 3.2, no jq guarantee on the
        // target), so _sdk_sample_port mirrors it. Guard the mirror.
        // Whitespace-normalised: the case arms are column-aligned in the script.
        var installer = System.Text.RegularExpressions.Regex.Replace(Read("install.sh"), @"[ \t]+", " ");
        foreach (var s in SdkSampleCatalog.Samples)
        {
            Assert.Contains($"{s.Id}) echo {s.Port} ;;", installer, StringComparison.Ordinal);
        }
        var langs = string.Join(' ', SdkSampleCatalog.Samples.Select(s => s.Id));
        Assert.Contains($"SDK_SAMPLE_LANGS=\"{langs}\"", installer);
    }

    [Fact]
    public void Unknown_ids_are_rejected_and_known_ones_normalize()
    {
        Assert.Null(SdkSampleCatalog.Find("cobol"));
        Assert.Null(SdkSampleCatalog.Find(""));
        Assert.Null(SdkSampleCatalog.Find(null));
        Assert.Equal("go", SdkSampleCatalog.Find("GO")?.Id);

        var ok = SdkSampleCatalog.NormalizeLanguages(["GO", "rust", "go"], out var unknown);
        Assert.Empty(unknown);
        // Catalog order, deduplicated — not request order.
        Assert.Equal(["rust", "go"], ok);

        SdkSampleCatalog.NormalizeLanguages(["go", "cobol"], out var bad);
        Assert.Equal(["cobol"], bad);
    }

    [Theory]
    [InlineData("{}")]
    [InlineData("""{"prefix_default":"/laghound","route_default":"/laghound/echo","samples":[]}""")]
    [InlineData("""{"prefix_default":"laghound","route_default":"/x","samples":[{"id":"a","port":1,"sdk_version":"1"}]}""")]
    [InlineData("""{"prefix_default":"/l","route_default":"/x","samples":[{"id":"a","port":1,"sdk_version":"1"},{"id":"a","port":2,"sdk_version":"1"}]}""")]
    [InlineData("""{"prefix_default":"/l","route_default":"/x","samples":[{"id":"a","port":1,"sdk_version":"1"},{"id":"b","port":1,"sdk_version":"1"}]}""")]
    [InlineData("""{"prefix_default":"/l","route_default":"/x","samples":[{"id":"a","port":1,"sdk_version":""}]}""")]
    public void Malformed_manifests_throw_loudly(string json)
    {
        Assert.ThrowsAny<Exception>(() => SdkSampleCatalog.Parse(json));
    }

    // ── Version re-derivation ────────────────────────────────────────────────

    private static string Version(string id) =>
        SdkSampleCatalog.Find(id)?.SdkVersion ?? throw new InvalidOperationException($"no catalog row for {id}");

    private static string JsPackageVersion()
    {
        using var doc = JsonDocument.Parse(Read("sdk", "js", "package.json"));
        return doc.RootElement.GetProperty("version").GetString()!;
    }

    private static string PythonVersion() =>
        Between(Read("sdk", "python", "src", "laghound", "_version.py"), "__version__ = \"", "\"");

    private static string RustCrateVersion()
    {
        // The [package] version, not the workspace marker above it.
        var text = Read("sdk", "rust", "Cargo.toml");
        var pkg = text.IndexOf("[package]", StringComparison.Ordinal);
        Assert.True(pkg >= 0, "sdk/rust/Cargo.toml has no [package] section");
        return Between(text[pkg..], "version = \"", "\"");
    }

    private static string GoConstVersion()
    {
        // Anchored on the whole const name: `ContractVersion = "v1"` sits two
        // lines above and would otherwise match a bare `Version = "` search.
        var m = System.Text.RegularExpressions.Regex.Match(
            Read("sdk", "go", "laghound.go"), @"(?m)^\s*Version\s*=\s*""([^""]+)""");
        Assert.True(m.Success, "sdk/go/laghound.go has no `Version = \"…\"` const");
        return m.Groups[1].Value;
    }

    private static string CsharpProjectVersion() =>
        Between(Read("sdk", "csharp", "LagHound.Endpoint", "LagHound.Endpoint.csproj"), "<Version>", "</Version>");

    private static string Between(string haystack, string start, string end)
    {
        var from = haystack.IndexOf(start, StringComparison.Ordinal);
        Assert.True(from >= 0, $"could not find '{start}'");
        from += start.Length;
        var to = haystack.IndexOf(end, from, StringComparison.Ordinal);
        Assert.True(to > from, $"could not find '{end}' after '{start}'");
        return haystack[from..to].Trim();
    }
}
