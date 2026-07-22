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

    // ── apibench (the measured /api/* workload suite) ─────────────────────

    private const string NetworkApibench = """
        {
          "id": "22222222-2222-2222-2222-222222222222",
          "endpoint": { "kind": "network", "host": "example.com", "port": 8443 },
          "workload": {
            "modes": ["http1", "apibench"],
            "runs": 10, "concurrency": 1, "timeout_ms": 5000,
            "payload_sizes": [], "capture_mode": "headers-only", "insecure": true
          }
        }
        """;

    [Fact]
    public void BuildArgs_strips_apibench_from_tester_modes()
    {
        var view = TestConfigView.From(Config(NetworkApibench));
        var args = RunExecutor.BuildArgs(view, "https://example.com:8443/health");
        var modesIdx = args.IndexOf("--modes");
        Assert.Equal("http1", args[modesIdx + 1]);
    }

    [Fact]
    public void Apibench_embedded_workload_set_loads_five_spec_workloads()
    {
        var names = ApibenchWorkloads.All.Select(w => w.Name).ToArray();
        Assert.Equal(
            new[] { "api-users", "api-transform", "api-aggregate", "api-search", "api-compress" },
            names);
        // Every path targets a spec-measured /api/* endpoint (API-SPEC.md §4).
        Assert.All(ApibenchWorkloads.All, w => Assert.StartsWith("/api/", w.Path));
        // POST workloads carry deterministic committed bodies.
        Assert.All(
            ApibenchWorkloads.All.Where(w => w.Method == "POST"),
            w => Assert.False(string.IsNullOrEmpty(w.Body)));
    }

    [Fact]
    public void Apibench_workload_target_rewrites_path_preserving_authority()
    {
        var search = ApibenchWorkloads.All.Single(w => w.Name == "api-search");
        var target = ApibenchWorkloads.WorkloadTarget("https://example.com:8443/health", search);
        Assert.Equal("https://example.com:8443/api/search?q=network&limit=10", target);
    }

    [Fact]
    public void Apibench_post_workload_args_carry_body_and_content_type()
    {
        var view = TestConfigView.From(Config(NetworkApibench));
        var transform = ApibenchWorkloads.All.Single(w => w.Name == "api-transform");
        var args = ApibenchWorkloads.BuildArgs(view, "https://example.com:8443/health", transform);

        Assert.Equal("--target", args[0]);
        Assert.Equal("https://example.com:8443/api/transform", args[1]);
        var modesIdx = args.IndexOf("--modes");
        Assert.Equal("http1", args[modesIdx + 1]);
        Assert.Contains("--insecure", args);

        var bodyIdx = args.IndexOf("--request-body");
        Assert.True(bodyIdx >= 0, "--request-body missing");
        // The body is the frozen dataset's transform_inputs[0] (§7 transform_input0).
        Assert.Contains("\"seed\":1", args[bodyIdx + 1]);
        var ctIdx = args.IndexOf("--request-content-type");
        Assert.Equal("application/json", args[ctIdx + 1]);
    }

    [Fact]
    public void Apibench_get_workload_args_have_no_body_flags()
    {
        var view = TestConfigView.From(Config(NetworkApibench));
        var users = ApibenchWorkloads.All.Single(w => w.Name == "api-users");
        var args = ApibenchWorkloads.BuildArgs(view, "https://example.com:8443/health", users);
        Assert.DoesNotContain("--request-body", args);
        Assert.DoesNotContain("--request-content-type", args);
    }

    [Fact]
    public void BuildArgs_no_payload_flag_when_not_needed_and_empty()
    {
        var view = TestConfigView.From(Config(NetworkDnsHttp2));
        Assert.DoesNotContain("--payload-sizes", RunExecutor.BuildArgs(view, "t"));
    }

    // ── sdkprobe (LagHound SDK) token + route args ─────────────────────────

    private const string SdkProbeConfig = """
        {
          "id": "33333333-3333-3333-3333-333333333333",
          "endpoint": { "kind": "network", "host": "https://customer.example.com" },
          "workload": {
            "modes": ["sdkprobe"],
            "runs": 10, "concurrency": 1, "timeout_ms": 30000,
            "payload_sizes": [], "insecure": false,
            "laghound_token": "secret-token-xyz",
            "laghound_route": "/laghound/health"
          }
        }
        """;

    [Fact]
    public void TestConfigView_reads_laghound_token_and_route_from_workload()
    {
        var view = TestConfigView.From(Config(SdkProbeConfig));
        Assert.Equal(new[] { "sdkprobe" }, view.Modes);
        Assert.Equal("secret-token-xyz", view.LagHoundToken);
        Assert.Equal("/laghound/health", view.LagHoundRoute);
    }

    [Fact]
    public void BuildArgs_appends_laghound_token_and_route_for_sdkprobe()
    {
        var view = TestConfigView.From(Config(SdkProbeConfig));
        var args = RunExecutor.BuildArgs(view, "https://customer.example.com");

        var ti = args.IndexOf("--laghound-token");
        Assert.True(ti >= 0, "--laghound-token missing");
        Assert.Equal("secret-token-xyz", args[ti + 1]);

        var ri = args.IndexOf("--laghound-route");
        Assert.True(ri >= 0, "--laghound-route missing");
        Assert.Equal("/laghound/health", args[ri + 1]);
    }

    [Fact]
    public void BuildArgs_omits_laghound_flags_when_no_token_present()
    {
        var view = TestConfigView.From(Config("""
            { "id":"00000000-0000-0000-0000-000000000000",
              "endpoint": { "kind":"network", "host":"h" },
              "workload": { "modes":["sdkprobe"], "runs":1, "concurrency":1, "timeout_ms":3000,
                            "payload_sizes":[], "insecure":false } }
            """));
        var args = RunExecutor.BuildArgs(view, "t");
        Assert.DoesNotContain("--laghound-token", args);
        Assert.DoesNotContain("--laghound-route", args);
    }

    [Fact]
    public void RedactSecretArgs_masks_token_values_but_keeps_flag_names()
    {
        var view = TestConfigView.From(Config(SdkProbeConfig));
        var args = RunExecutor.BuildArgs(view, "https://customer.example.com");

        var redacted = RunExecutor.RedactSecretArgs(args);
        var line = string.Join(" ", redacted);

        // The flag name survives; the secret value never appears in the log line.
        Assert.Contains("--laghound-token", line);
        Assert.Contains("***REDACTED***", line);
        Assert.DoesNotContain("secret-token-xyz", line);
        // The non-secret route value is NOT masked.
        Assert.Contains("/laghound/health", line);
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

    // ── Overall run deadline (quality audit F4) ─────────────────────────────

    [Fact]
    public void MaxDurationSecs_is_read_from_config_root_not_workload()
    {
        var view = TestConfigView.From(Config("""
            { "id":"00000000-0000-0000-0000-000000000000",
              "endpoint": { "kind":"network", "host":"h" },
              "workload": { "modes":["http1"], "runs":1, "concurrency":1, "timeout_ms":3000,
                            "payload_sizes":[], "insecure":false },
              "max_duration_secs": 900 }
            """));
        Assert.Equal(900u, view.MaxDurationSecs);
    }

    [Fact]
    public void MaxDurationSecs_defaults_to_zero_when_absent_or_null()
    {
        Assert.Equal(0u, TestConfigView.From(Config(NetworkDnsHttp2)).MaxDurationSecs);

        var withNull = TestConfigView.From(Config("""
            { "id":"00000000-0000-0000-0000-000000000000",
              "endpoint": { "kind":"network", "host":"h" },
              "workload": { "modes":["http1"], "runs":1, "concurrency":1, "timeout_ms":3000,
                            "payload_sizes":[], "insecure":false },
              "max_duration_secs": null }
            """));
        Assert.Equal(0u, withNull.MaxDurationSecs);
    }

    [Fact]
    public void InvocationDeadline_prefers_config_max_duration_plus_slack()
    {
        var view = TestConfigView.From(Config("""
            { "id":"00000000-0000-0000-0000-000000000000",
              "endpoint": { "kind":"network", "host":"h" },
              "workload": { "modes":["http1"], "runs":1000, "concurrency":1, "timeout_ms":3000,
                            "payload_sizes":[], "insecure":false },
              "max_duration_secs": 600 }
            """));
        // max_duration_secs (600) + the 60s slack — the workload arithmetic is
        // ignored when the config carries its own budget.
        Assert.Equal(TimeSpan.FromSeconds(660), RunExecutor.ComputeInvocationDeadline(view));
    }

    [Fact]
    public void InvocationDeadline_falls_back_to_timeout_times_runs_times_modes()
    {
        // NetworkDnsHttp2: timeout 5000ms→5s, runs=10, modes=4 → 5*10*4 + 60 slack.
        var view = TestConfigView.From(Config(NetworkDnsHttp2));
        Assert.Equal(TimeSpan.FromSeconds(5 * 10 * 4 + 60), RunExecutor.ComputeInvocationDeadline(view));
    }

    [Fact]
    public void InvocationDeadline_is_clamped_to_24_hours()
    {
        var view = TestConfigView.From(Config("""
            { "id":"00000000-0000-0000-0000-000000000000",
              "endpoint": { "kind":"network", "host":"h" },
              "workload": { "modes":["http1"], "runs":4000000, "concurrency":1, "timeout_ms":60000,
                            "payload_sizes":[], "insecure":false } }
            """));
        Assert.Equal(TimeSpan.FromHours(24), RunExecutor.ComputeInvocationDeadline(view));
    }
}
