using System.Text.Json;
using Networker.Agent;

namespace Networker.Agent.Tests;

/// <summary>
/// Config parse + CLI-arg construction — the C# executor must build byte-for-byte
/// the same <c>networker-tester</c> command line the Rust <c>build_args</c> /
/// <c>endpoint_to_target</c> produce.
/// </summary>
public class ConfigAndArgsTests
{
    private static JsonElement Config(string json) => JsonDocument.Parse(json).RootElement.Clone();

    private const string NetworkDnsHttp2 = """
        {
          "id": "11111111-1111-1111-1111-111111111111",
          "endpoint": { "kind": "network", "host": "www.cloudflare.com" },
          "workload": {
            "modes": ["dns", "tcp", "tls", "http2"],
            "runs": 10, "concurrency": 1, "timeout_ms": 5000,
            "payload_sizes": [], "capture_mode": "headers-only", "insecure": false
          }
        }
        """;

    [Fact]
    public void Parses_network_endpoint_and_workload()
    {
        var view = TestConfigView.From(Config(NetworkDnsHttp2));

        Assert.Equal("network", view.EndpointKind);
        Assert.NotNull(view.Network);
        Assert.Equal("www.cloudflare.com", view.Network!.Host);
        Assert.Null(view.Network.Port);
        Assert.Equal(new[] { "dns", "tcp", "tls", "http2" }, view.Modes);
        Assert.Equal(10u, view.Runs);
        Assert.Equal(1u, view.Concurrency);
        Assert.Equal(5000u, view.TimeoutMs);
        Assert.False(view.Insecure);
        Assert.False(view.IsBenchmark);
    }

    [Fact]
    public void EndpointToTarget_network_no_port_uses_https_health()
    {
        var view = TestConfigView.From(Config(NetworkDnsHttp2));
        Assert.Equal("https://www.cloudflare.com/health", RunExecutor.EndpointToTarget(view));
    }

    [Fact]
    public void EndpointToTarget_network_with_port_includes_port()
    {
        var view = TestConfigView.From(Config("""
            { "id":"00000000-0000-0000-0000-000000000000",
              "endpoint": { "kind":"network", "host":"example.com", "port":8443 },
              "workload": { "modes":["http1"], "runs":1, "concurrency":1, "timeout_ms":3000,
                            "payload_sizes":[], "capture_mode":"headers-only", "insecure":false } }
            """));
        Assert.Equal("https://example.com:8443/health", RunExecutor.EndpointToTarget(view));
    }

    [Fact]
    public void EndpointToTarget_passes_through_full_url_host()
    {
        var view = TestConfigView.From(Config("""
            { "id":"00000000-0000-0000-0000-000000000000",
              "endpoint": { "kind":"network", "host":"https://api.example.com/v1" },
              "workload": { "modes":["http1"], "runs":1, "concurrency":1, "timeout_ms":3000,
                            "payload_sizes":[], "capture_mode":"headers-only", "insecure":false } }
            """));
        Assert.Equal("https://api.example.com/v1", RunExecutor.EndpointToTarget(view));
    }

    [Theory]
    [InlineData("proxy", """{ "kind":"proxy", "proxy_endpoint_id":"00000000-0000-0000-0000-000000000001" }""")]
    [InlineData("runtime", """{ "kind":"runtime", "runtime_id":"00000000-0000-0000-0000-000000000001", "language":"rust" }""")]
    [InlineData("pending", """{ "kind":"pending", "cloud_account_id":"00000000-0000-0000-0000-000000000001", "region":"eastus", "vm_size":"x", "os":"linux", "proxy_stack":"nginx", "topology":"loopback" }""")]
    public void EndpointToTarget_unsupported_kinds_return_null(string kind, string endpointJson)
    {
        var view = TestConfigView.From(Config($$"""
            { "id":"00000000-0000-0000-0000-000000000000",
              "endpoint": {{endpointJson}},
              "workload": { "modes":["http1"], "runs":1, "concurrency":1, "timeout_ms":3000,
                            "payload_sizes":[], "capture_mode":"headers-only", "insecure":false } }
            """));
        Assert.Equal(kind, view.EndpointKind);
        Assert.Null(RunExecutor.EndpointToTarget(view));
    }

    [Fact]
    public void BuildArgs_matches_rust_layout()
    {
        var view = TestConfigView.From(Config(NetworkDnsHttp2));
        var args = RunExecutor.BuildArgs(view, "https://www.cloudflare.com/health");

        Assert.Equal(new[]
        {
            "--target", "https://www.cloudflare.com/health",
            "--modes", "dns,tcp,tls,http2",
            "--runs", "10",
            "--concurrency", "1",
            "--timeout", "5", // 5000ms → 5s
            "--json-stdout",
        }, args);
    }

    [Fact]
    public void BuildArgs_timeout_rounds_up_and_floors_at_one()
    {
        var view = TestConfigView.From(Config("""
            { "id":"00000000-0000-0000-0000-000000000000",
              "endpoint": { "kind":"network", "host":"h" },
              "workload": { "modes":["http1"], "runs":1, "concurrency":1, "timeout_ms":1500,
                            "payload_sizes":[], "capture_mode":"headers-only", "insecure":false } }
            """));
        var args = RunExecutor.BuildArgs(view, "t");
        var i = args.IndexOf("--timeout");
        Assert.Equal("2", args[i + 1]); // 1500ms → ceil → 2s

        var zero = TestConfigView.From(Config("""
            { "id":"00000000-0000-0000-0000-000000000000",
              "endpoint": { "kind":"network", "host":"h" },
              "workload": { "modes":["http1"], "runs":1, "concurrency":1, "timeout_ms":0,
                            "payload_sizes":[], "capture_mode":"headers-only", "insecure":false } }
            """));
        var zargs = RunExecutor.BuildArgs(zero, "t");
        Assert.Equal("1", zargs[zargs.IndexOf("--timeout") + 1]); // floor 1
    }

    [Fact]
    public void BuildArgs_appends_insecure_when_set()
    {
        var view = TestConfigView.From(Config("""
            { "id":"00000000-0000-0000-0000-000000000000",
              "endpoint": { "kind":"network", "host":"h" },
              "workload": { "modes":["http1"], "runs":1, "concurrency":1, "timeout_ms":3000,
                            "payload_sizes":[], "capture_mode":"headers-only", "insecure":true } }
            """));
        Assert.Contains("--insecure", RunExecutor.BuildArgs(view, "t"));
    }

    [Fact]
    public void BuildArgs_download_without_sizes_falls_back_to_65536()
    {
        var view = TestConfigView.From(Config("""
            { "id":"00000000-0000-0000-0000-000000000000",
              "endpoint": { "kind":"network", "host":"h" },
              "workload": { "modes":["download"], "runs":1, "concurrency":1, "timeout_ms":3000,
                            "payload_sizes":[], "capture_mode":"headers-only", "insecure":false } }
            """));
        var args = RunExecutor.BuildArgs(view, "t");
        var i = args.IndexOf("--payload-sizes");
        Assert.True(i >= 0);
        Assert.Equal("65536", args[i + 1]);
    }

    [Fact]
    public void BuildArgs_respects_explicit_payload_sizes()
    {
        var view = TestConfigView.From(Config("""
            { "id":"00000000-0000-0000-0000-000000000000",
              "endpoint": { "kind":"network", "host":"h" },
              "workload": { "modes":["upload"], "runs":1, "concurrency":1, "timeout_ms":3000,
                            "payload_sizes":[1024, 4096], "capture_mode":"headers-only", "insecure":false } }
            """));
        var args = RunExecutor.BuildArgs(view, "t");
        var i = args.IndexOf("--payload-sizes");
        Assert.Equal("1024,4096", args[i + 1]);
    }

    [Fact]
    public void BuildArgs_no_payload_flag_when_not_needed_and_empty()
    {
        var view = TestConfigView.From(Config(NetworkDnsHttp2));
        Assert.DoesNotContain("--payload-sizes", RunExecutor.BuildArgs(view, "t"));
    }

    [Fact]
    public void Methodology_presence_flags_benchmark_mode()
    {
        var view = TestConfigView.From(Config("""
            { "id":"00000000-0000-0000-0000-000000000000",
              "endpoint": { "kind":"network", "host":"h" },
              "workload": { "modes":["http1"], "runs":1, "concurrency":1, "timeout_ms":3000,
                            "payload_sizes":[], "capture_mode":"headers-only", "insecure":false },
              "methodology": { "warmup_runs": 5, "measured_runs": 30 } }
            """));
        Assert.True(view.IsBenchmark);
    }

    [Fact]
    public void BuildUri_appends_key_query()
    {
        var uri = RawWebSocketClient.BuildUri("ws://localhost:3000/ws/agent", "dev-key");
        Assert.Equal("ws://localhost:3000/ws/agent?key=dev-key", uri.ToString());
    }

    [Fact]
    public void BuildUri_uses_ampersand_when_query_present()
    {
        var uri = RawWebSocketClient.BuildUri("ws://host/ws/agent?foo=1", "k");
        Assert.Equal("ws://host/ws/agent?foo=1&key=k", uri.ToString());
    }

    [Fact]
    public void RustEnvFallbacks_fill_unset_values()
    {
        var opts = new AgentOptions();
        opts.ApplyRustEnvFallbacks(new Dictionary<string, string?>
        {
            ["AGENT_DASHBOARD_URL"] = "ws://dash:9/ws/agent",
            ["AGENT_API_KEY"] = "secret",
            ["AGENT_TESTER_PATH"] = "/opt/networker-tester",
        });
        Assert.Equal("ws://dash:9/ws/agent", opts.DashboardUrl);
        Assert.Equal("secret", opts.ApiKey);
        Assert.Equal("/opt/networker-tester", opts.TesterPath);
    }

    [Fact]
    public void RustEnvFallbacks_do_not_override_explicit_values()
    {
        var opts = new AgentOptions { DashboardUrl = "ws://explicit/ws/agent", ApiKey = "explicit-key" };
        opts.ApplyRustEnvFallbacks(new Dictionary<string, string?>
        {
            ["AGENT_DASHBOARD_URL"] = "ws://fallback/ws/agent",
            ["AGENT_API_KEY"] = "fallback-key",
        });
        Assert.Equal("ws://explicit/ws/agent", opts.DashboardUrl);
        Assert.Equal("explicit-key", opts.ApiKey);
    }
}
