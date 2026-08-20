using System.Text.Json;
using System.Text.Json.Nodes;
using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins the wire shape of <c>GET /api/v2/test-runs/{id}/attempts</c> (audit
/// F3): the <c>{"attempts":[...]}</c> envelope the legacy Rust handler
/// returned and the frontend client types, and the per-attempt snake_case
/// field set that mirrors the tester's <c>RequestAttempt</c> table and the
/// frontend <c>Attempt</c> type.
/// </summary>
public sealed class TestRunsContractTests
{
    private static readonly JsonSerializerOptions WebOptions =
        new(JsonSerializerDefaults.Web);

    private static AttemptView SampleAttempt() => new(
        AttemptId: Guid.Parse("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
        Protocol: "http2",
        SequenceNum: 3,
        StartedAt: new DateTime(2026, 7, 14, 1, 22, 24, DateTimeKind.Utc),
        FinishedAt: new DateTime(2026, 7, 14, 1, 22, 25, DateTimeKind.Utc),
        Success: false,
        ErrorMessage: "connection refused (os error 111)",
        RetryCount: 1);

    [Fact]
    public void Attempts_response_is_an_object_with_a_top_level_attempts_array()
    {
        var json = JsonSerializer.Serialize(
            new AttemptListResponse(new[] { SampleAttempt() }), WebOptions);
        var root = JsonNode.Parse(json)!.AsObject();

        Assert.Single(root);
        Assert.True(root.ContainsKey("attempts"));
        Assert.IsType<JsonArray>(root["attempts"]);
        Assert.Single(root["attempts"]!.AsArray());
    }

    [Fact]
    public void Empty_attempts_still_serializes_the_envelope()
    {
        // An existing run with no probe rows is 200 + `{"attempts":[]}` —
        // never a 404 (the audit-F3 dead end) and never a bare `[]`.
        var json = JsonSerializer.Serialize(
            new AttemptListResponse(Array.Empty<AttemptView>()), WebOptions);

        Assert.Equal("""{"attempts":[]}""", json);
    }

    [Fact]
    public void Attempt_item_emits_the_exact_snake_case_field_set()
    {
        var json = JsonSerializer.Serialize(SampleAttempt(), WebOptions);
        var item = JsonNode.Parse(json)!.AsObject();

        // sample_index (#782 P2) joins retry_count as an always-emitted int:
        // 0 on every non-burst attempt, which is the truthful value (a logical
        // attempt without a burst has exactly one sample), so the frontend can
        // read it unconditionally instead of handling a third "absent" state.
        var expected = new[]
        {
            "attempt_id", "protocol", "sequence_num", "started_at",
            "finished_at", "success", "error_message", "retry_count",
            "sample_index",
        };

        Assert.Equal(expected, item.Select(p => p.Key).ToArray());
        Assert.Equal("http2", item["protocol"]!.GetValue<string>());
        Assert.Equal(3, item["sequence_num"]!.GetValue<int>());
        Assert.False(item["success"]!.GetValue<bool>());
        Assert.Equal(1, item["retry_count"]!.GetValue<int>());
        Assert.Equal(0, item["sample_index"]!.GetValue<int>());
    }

    [Fact]
    public void Attempt_item_carries_the_burst_sample_index()
    {
        // A burst attempt (#782 P2) reports WHICH sample it is, separately
        // from how many retries that sample needed — the two must never be
        // conflated: a retry replaces a failed try, a sample is a repeat.
        var burst = SampleAttempt() with { SampleIndex = 4, RetryCount = 1 };
        var item = JsonNode.Parse(JsonSerializer.Serialize(burst, WebOptions))!.AsObject();

        Assert.Equal(4, item["sample_index"]!.GetValue<int>());
        Assert.Equal(1, item["retry_count"]!.GetValue<int>());
    }

    [Fact]
    public void Absent_phase_objects_are_omitted_not_null()
    {
        // Backward compatibility: an attempt without persisted phase rows must
        // serialize with the EXACT pre-widening field set (asserted above) —
        // no `"dns": null` noise. This is what keeps old runs' wire shape
        // byte-identical after the phase-detail widening.
        var json = JsonSerializer.Serialize(SampleAttempt(), WebOptions);
        var item = JsonNode.Parse(json)!.AsObject();

        foreach (var phase in new[] { "dns", "tcp", "tls", "http", "udp", "server_timing" })
        {
            Assert.False(item.ContainsKey(phase), $"absent phase '{phase}' must be omitted");
        }
    }

    [Fact]
    public void Phase_objects_emit_the_tester_snake_case_field_names()
    {
        // The nested phase objects must use the SAME snake_case names as the
        // tester's live JSON (crates/networker-tester/src/metrics.rs) so the
        // frontend LiveAttempt type renders REST and live attempts through one
        // code path. A rename here silently blanks the run-detail phase cards.
        var full = SampleAttempt() with
        {
            Dns = new AttemptDnsView(1.5, true, "example.com", new[] { "93.184.216.34" }),
            Tcp = new AttemptTcpView(2.5, "93.184.216.34:443", 1448, 12.5, 0, 3, 10, "bbr", 1250000, 11.9),
            Tls = new AttemptTlsView(9.1, "TLSv1_3", "TLS13_AES_256_GCM_SHA384", "h2",
                new DateTime(2027, 1, 1, 0, 0, 0, DateTimeKind.Utc)),
            Http = new AttemptHttpView(200, "HTTP/2.0", 20.5, 180.0, 10485760, 0, 10485760L, 41.7),
            Udp = new AttemptUdpView(3.4, 2.1, 6.7, 0.9, 2.5, 40, 39),
            ServerTiming = new AttemptServerTimingView(7.9, 1.2, 9.3),
        };
        var item = JsonNode.Parse(JsonSerializer.Serialize(full, WebOptions))!.AsObject();

        Assert.Equal(
            new[] { "duration_ms", "success", "query_name", "resolved_ips" },
            item["dns"]!.AsObject().Select(p => p.Key).ToArray());
        Assert.Equal(
            new[]
            {
                "connect_duration_ms", "remote_addr", "mss_bytes", "rtt_estimate_ms",
                "retransmits", "total_retrans", "snd_cwnd", "congestion_algorithm",
                "delivery_rate_bps", "min_rtt_ms",
            },
            item["tcp"]!.AsObject().Select(p => p.Key).ToArray());
        Assert.Equal(
            new[]
            {
                "handshake_duration_ms", "protocol_version", "cipher_suite",
                "alpn_negotiated", "cert_expiry",
            },
            item["tls"]!.AsObject().Select(p => p.Key).ToArray());
        Assert.Equal(
            new[]
            {
                "status_code", "negotiated_version", "ttfb_ms", "total_duration_ms",
                "body_size_bytes", "redirect_count", "payload_bytes", "throughput_mbps",
            },
            item["http"]!.AsObject().Select(p => p.Key).ToArray());
        Assert.Equal(
            new[]
            {
                "rtt_avg_ms", "rtt_min_ms", "rtt_p95_ms", "jitter_ms",
                "loss_percent", "probe_count", "success_count",
            },
            item["udp"]!.AsObject().Select(p => p.Key).ToArray());
        Assert.Equal(
            new[] { "processing_ms", "recv_body_ms", "total_server_ms" },
            item["server_timing"]!.AsObject().Select(p => p.Key).ToArray());

        Assert.Equal("bbr", item["tcp"]!["congestion_algorithm"]!.GetValue<string>());
        Assert.Equal(41.7, item["http"]!["throughput_mbps"]!.GetValue<double>());
        Assert.Equal(0.9, item["udp"]!["jitter_ms"]!.GetValue<double>());
    }

    // ── Run detail: envelope pass-through (V046) ────────────────────────────

    private static TestRunsEndpoints.RunDetailRow SampleRunDetail(string? envelope) => new(
        Id: Guid.Parse("11111111-2222-3333-4444-555555555555"),
        TestConfigId: Guid.Parse("66666666-7777-8888-9999-aaaaaaaaaaaa"),
        ProjectId: "p1",
        Status: "completed",
        StartedAt: new DateTime(2026, 7, 14, 1, 22, 0, DateTimeKind.Utc),
        FinishedAt: new DateTime(2026, 7, 14, 1, 23, 0, DateTimeKind.Utc),
        SuccessCount: 10,
        FailureCount: 0,
        ErrorMessage: null,
        ArtifactId: null,
        TesterId: null,
        WorkerId: "worker-1",
        LastHeartbeat: null,
        CreatedAt: new DateTime(2026, 7, 14, 1, 21, 0, DateTimeKind.Utc),
        ComparisonGroupId: null,
        ClientEnvelope: envelope);

    /// The exact pre-envelope field set of GET /api/v2/test-runs/{id} — the
    /// wire shape every consumer saw before V046, which runs WITHOUT a stored
    /// envelope must keep serving byte-identically.
    private static readonly string[] RunDetailBaseFields =
    {
        "id", "test_config_id", "project_id", "status", "result_status",
        "started_at", "finished_at", "success_count", "failure_count",
        "error_message", "artifact_id", "tester_id", "worker_id",
        "last_heartbeat", "created_at", "comparison_group_id",
    };

    [Fact]
    public void Run_detail_without_envelope_pins_the_pre_envelope_field_set()
    {
        // Old runs (and runs finished by pre-envelope agents) have a NULL
        // client_envelope — the response must not gain an `"envelope": null`
        // member; the old wire shape is unchanged.
        var json = JsonSerializer.Serialize(
            TestRunsEndpoints.BuildRunDetail(SampleRunDetail(envelope: null)), WebOptions);
        var root = JsonNode.Parse(json)!.AsObject();

        Assert.Equal(RunDetailBaseFields, root.Select(p => p.Key).ToArray());
        Assert.False(root.ContainsKey("envelope"));
    }

    [Fact]
    public void Run_detail_with_envelope_serves_it_verbatim_as_raw_json()
    {
        // Stored compact JSON (snake_case, as ingested from the agent's
        // run_finished) must re-emit as raw JSON — parsed pass-through, not an
        // escaped string — appended after the pinned base field set.
        const string stored = """
            {"client_geo":{"country":"US","asn":13335,"as_org":"Cloudflare"},"clock_sync":{"offset_ms":-3.2},"client_load_before":{"load_avg_1m":0.42},"client_info":{"os":"linux","cpu_cores":4}}
            """;
        var json = JsonSerializer.Serialize(
            TestRunsEndpoints.BuildRunDetail(SampleRunDetail(stored.Trim())), WebOptions);
        var root = JsonNode.Parse(json)!.AsObject();

        Assert.Equal(
            RunDetailBaseFields.Append("envelope").ToArray(),
            root.Select(p => p.Key).ToArray());

        var envelope = root["envelope"]!.AsObject();
        Assert.Equal("US", envelope["client_geo"]!["country"]!.GetValue<string>());
        Assert.Equal(13335, envelope["client_geo"]!["asn"]!.GetValue<int>());
        Assert.Equal(-3.2, envelope["clock_sync"]!["offset_ms"]!.GetValue<double>());
        Assert.Equal(0.42, envelope["client_load_before"]!["load_avg_1m"]!.GetValue<double>());
        Assert.Equal(4, envelope["client_info"]!["cpu_cores"]!.GetValue<int>());
    }

    // ── Run list: runner identity for provider / capacity grouping ──────────

    private static TestRunsEndpoints.RunListRow SampleRunListRow(
        string? cloud, string? region, string? vmSize) => new(
        Id: Guid.Parse("11111111-2222-3333-4444-555555555555"),
        TestConfigId: Guid.Parse("66666666-7777-8888-9999-aaaaaaaaaaaa"),
        ProjectId: "p1",
        Status: "completed",
        StartedAt: new DateTime(2026, 8, 20, 1, 22, 0, DateTimeKind.Utc),
        FinishedAt: new DateTime(2026, 8, 20, 1, 23, 0, DateTimeKind.Utc),
        SuccessCount: 4,
        FailureCount: 0,
        ErrorMessage: null,
        ArtifactId: null,
        TesterId: cloud is null ? null : Guid.Parse("bbbbbbbb-cccc-dddd-eeee-ffffffffffff"),
        WorkerId: "worker-1",
        LastHeartbeat: null,
        CreatedAt: new DateTime(2026, 8, 20, 1, 21, 0, DateTimeKind.Utc),
        ComparisonGroupId: null,
        ConfigName: "Diag: microsoft.com (Quick)",
        EndpointKind: "network",
        TestKind: "url_probe",
        Workload: """{"modes":["dns","tcp","tls","http2"]}""",
        RunnerCloud: cloud,
        RunnerRegion: region,
        RunnerVmSize: vmSize);

    /// The list item's field set: the pre-existing list shape (base run +
    /// config join + computed result_status/modes) followed by the ADDITIVE
    /// runner identity fields. Appended, never interleaved, so a consumer
    /// reading by position keeps working.
    private static readonly string[] RunListFields =
    {
        "id", "test_config_id", "project_id", "status", "result_status",
        "started_at", "finished_at", "success_count", "failure_count",
        "error_message", "artifact_id", "tester_id", "worker_id",
        "last_heartbeat", "created_at", "comparison_group_id",
        "config_name", "endpoint_kind", "test_kind", "modes",
        "runner_cloud", "runner_region", "runner_vm_size",
        "runner_vcpus", "runner_memory_gb",
    };

    [Fact]
    public void Run_list_item_pins_the_field_set_with_runner_identity_appended()
    {
        var json = JsonSerializer.Serialize(
            TestRunsEndpoints.BuildRunListItem(SampleRunListRow("gcp", "us-central1", "e2-medium")),
            WebOptions);
        var root = JsonNode.Parse(json)!.AsObject();

        Assert.Equal(RunListFields, root.Select(p => p.Key).ToArray());
        Assert.Equal("completed", root["result_status"]!.GetValue<string>());
        Assert.Equal(
            new[] { "dns", "tcp", "tls", "http2" },
            root["modes"]!.AsArray().Select(m => m!.GetValue<string>()).ToArray());
    }

    [Fact]
    public void Run_list_item_resolves_runner_specs_from_the_vm_catalog()
    {
        // The probe page's capacity axis: cloud + size straight from the
        // tester row, vCPU / memory from VmNetworkSpecs so every list
        // consumer sees the same numbers the infra envelope shows.
        var json = JsonSerializer.Serialize(
            TestRunsEndpoints.BuildRunListItem(SampleRunListRow("gcp", "us-central1", "e2-medium")),
            WebOptions);
        var root = JsonNode.Parse(json)!.AsObject();

        Assert.Equal("gcp", root["runner_cloud"]!.GetValue<string>());
        Assert.Equal("us-central1", root["runner_region"]!.GetValue<string>());
        Assert.Equal("e2-medium", root["runner_vm_size"]!.GetValue<string>());
        Assert.Equal(2, root["runner_vcpus"]!.GetValue<int>());
        Assert.Equal(4.0, root["runner_memory_gb"]!.GetValue<double>());
    }

    [Fact]
    public void Run_list_item_without_a_tester_emits_null_runner_fields()
    {
        // No tester (standalone agent) or a deleted tester (tester_id is
        // ON DELETE SET NULL): identity and specs are null, the keys stay so
        // the frontend's "unknown runner" bucket is an explicit null, never a
        // missing-vs-present ambiguity.
        var json = JsonSerializer.Serialize(
            TestRunsEndpoints.BuildRunListItem(SampleRunListRow(null, null, null)),
            WebOptions);
        var root = JsonNode.Parse(json)!.AsObject();

        Assert.Equal(RunListFields, root.Select(p => p.Key).ToArray());
        foreach (var field in new[] { "tester_id", "runner_cloud", "runner_region", "runner_vm_size", "runner_vcpus", "runner_memory_gb" })
        {
            Assert.Null(root[field]);
        }
    }

    [Fact]
    public void Run_list_item_with_an_uncatalogued_size_keeps_identity_but_nulls_specs()
    {
        // A size the catalog does not know (custom / brand-new SKU): the
        // provider axis still works (cloud + size pass through), only the
        // capacity numbers are unknown.
        var json = JsonSerializer.Serialize(
            TestRunsEndpoints.BuildRunListItem(SampleRunListRow("azure", "westeurope", "Standard_Z99_v9")),
            WebOptions);
        var root = JsonNode.Parse(json)!.AsObject();

        Assert.Equal("azure", root["runner_cloud"]!.GetValue<string>());
        Assert.Equal("Standard_Z99_v9", root["runner_vm_size"]!.GetValue<string>());
        Assert.Null(root["runner_vcpus"]);
        Assert.Null(root["runner_memory_gb"]);
    }

    [Fact]
    public void Null_optional_phase_fields_are_omitted_within_a_phase()
    {
        // Kernel TCP stats are best-effort (null on Windows testers / old
        // kernels) — a null column is omitted, mirroring the tester's
        // skip_serializing_if on the live path.
        var withTcp = SampleAttempt() with
        {
            Tcp = new AttemptTcpView(2.5, "93.184.216.34:443", null, null, null, null, null, null, null, null),
        };
        var tcp = JsonNode.Parse(JsonSerializer.Serialize(withTcp, WebOptions))!
            .AsObject()["tcp"]!.AsObject();

        Assert.Equal(new[] { "connect_duration_ms", "remote_addr" }, tcp.Select(p => p.Key).ToArray());
    }
}
