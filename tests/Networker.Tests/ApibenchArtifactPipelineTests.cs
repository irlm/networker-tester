using System.Net;
using System.Net.Http.Headers;
using System.Net.WebSockets;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.Logging.Abstractions;
using Networker.Agent;
using Networker.ControlPlane.Provisioning;
using Networker.ControlPlane.Security;
using Networker.Data;
using Networker.Data.Entities;
using Xunit;

namespace Networker.Tests;

/// <summary>
/// Issue #796: apibench comparison runs completed 60/60 in prod with an
/// artifact row per run — but <c>cases: []</c> and a <c>{success,failure}</c>
/// summaries object, so the per-case apibench results (api-users /
/// api-transform / api-aggregate / api-search / api-compress p50/p95/success)
/// were unretrievable from <c>GET /api/v2/test-runs/{id}/artifact</c>, and the
/// summaries shape silently disabled <c>RegressionAnalyzer</c> for every
/// agent-executed benchmark run.
///
/// <para>This drives the REAL cross-layer chain: the real
/// <see cref="RunExecutor"/> expands an apibench benchmark config into
/// per-workload tester invocations against a fake tester, its terminal
/// <c>run_finished</c> frame — serialized by the agent's own wire encoder — is
/// ingested over a real <c>/ws/agent</c> socket into real Postgres, and the
/// artifact read endpoint must then serve populated per-case
/// <c>cases</c>/<c>summaries</c> that the regression analyzer can parse.</para>
/// </summary>
public class ApibenchArtifactPipelineTests : IClassFixture<ControlPlaneFixture>, IDisposable
{
    private readonly ControlPlaneFixture _fx;
    private readonly string _scratch = Directory.CreateTempSubdirectory("apibench-e2e").FullName;

    public ApibenchArtifactPipelineTests(ControlPlaneFixture fx) => _fx = fx;

    public void Dispose()
    {
        try { Directory.Delete(_scratch, recursive: true); } catch { /* best-effort */ }
    }

    private const string Pid = ControlPlaneFixture.SeededProjectId;

    // ── Agent-side half: real RunExecutor + fake tester ──────────────────────

    private sealed class CollectingSink : RawWebSocketClient.IFrameSink
    {
        public List<AgentMessage> Messages { get; } = [];
        public bool TrySend(AgentMessage message) { Messages.Add(message); return true; }
        public bool TrySendLowPriority(AgentMessage message) => TrySend(message);
    }

    /// <summary>Fake networker-tester: prints the same apibench-shaped TestRun
    /// JSON for every workload invocation (2 successes at 10ms/20ms + 1
    /// failure) and exits 0. Cross-platform (sh / cmd).</summary>
    private string WriteFakeTester()
    {
        var stdout = """
            {"schema_version":"1.0",
             "attempts":[{"attempt_id":"a1","protocol":"http1","success":true,
                          "http":{"total_duration_ms":10.0}},
                         {"attempt_id":"a2","protocol":"http1","success":true,
                          "http":{"total_duration_ms":20.0}},
                         {"attempt_id":"a3","protocol":"http1","success":false}]}
            """.ReplaceLineEndings(" ");
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
        {
            var path = Path.Combine(_scratch, "networker-tester.cmd");
            File.WriteAllText(path, "@echo off\r\necho " + stdout
                .Replace("%", "%%").Replace("^", "^^").Replace("&", "^&")
                .Replace("<", "^<").Replace(">", "^>").Replace("|", "^|") + "\r\nexit /b 0\r\n");
            return path;
        }
        else
        {
            var path = Path.Combine(_scratch, "networker-tester");
            File.WriteAllText(path, "#!/bin/sh\ncat <<'NWEOF'\n" + stdout + "\nNWEOF\nexit 0\n");
            var psi = new System.Diagnostics.ProcessStartInfo("chmod", $"+x \"{path}\"") { UseShellExecute = false };
            System.Diagnostics.Process.Start(psi)!.WaitForExit();
            return path;
        }
    }

    private async Task<RunFinishedMessage> ExecuteApibenchRunAsync(Guid runId)
    {
        var exec = new RunExecutor(
            NullLogger<RunExecutor>.Instance,
            new AgentOptions { TesterPath = WriteFakeTester() });
        var sink = new CollectingSink();
        var config = JsonDocument.Parse("""
            { "id":"66666666-6666-4666-8666-666666666666",
              "endpoint": { "kind":"network", "host":"example.com", "port":8443 },
              "workload": { "modes":["apibench"], "runs":3, "concurrency":1, "timeout_ms":3000,
                            "payload_sizes":[], "capture_mode":"headers-only", "insecure":true },
              "methodology": { "warmup_runs":0, "measured_runs":3 } }
            """).RootElement.Clone();

        await exec.ExecuteAsync(runId, config, sink, CancellationToken.None);

        var finished = Assert.IsType<RunFinishedMessage>(sink.Messages[^1]);
        Assert.Equal("completed", finished.Status);
        Assert.NotNull(finished.Artifact);
        return finished;
    }

    // ── Control-plane half: seed run, real WS ingest, HTTP read-back ─────────

    private async Task<(Guid AgentId, string ApiKey, Guid RunId)> SeedAgentWithRunningRunAsync()
    {
        var agentId = Guid.NewGuid();
        var apiKey = $"itest-apibench-{Guid.NewGuid():N}";
        var runId = Guid.NewGuid();
        var cfgId = Guid.NewGuid();
        var now = DateTime.UtcNow;

        await using var db = _fx.NewDbContext();
        db.Agents.Add(new Networker.Data.Entities.Agent
        {
            AgentId = agentId,
            Name = $"apibench-{agentId:N}"[..40],
            ProjectId = Pid,
            Status = "offline",
            Region = "eastus",
            Provider = "azure",
            Version = "0.28.237",
            RegisteredAt = now.AddHours(-1),
            LastHeartbeat = now,
            ApiKeyHash = AgentApiKeys.HashHex(apiKey),
        });
        db.TestConfigs.Add(new TestConfig
        {
            Id = cfgId,
            ProjectId = Pid,
            Name = $"apibench-{cfgId:N}",
            EndpointKind = "network",
            EndpointRef = """{"kind":"network","host":"10.0.0.7","port":8443}""",
            Workload = """{"modes":["apibench"],"runs":3}""",
            Methodology = """{"warmup_runs":0,"measured_runs":3}""",
            MaxDurationSecs = 1800,
            CreatedAt = now.AddMinutes(-30),
            UpdatedAt = now.AddMinutes(-30),
        });
        db.TestRuns.Add(new TestRun
        {
            Id = runId,
            TestConfigId = cfgId,
            ProjectId = Pid,
            Status = "running",
            WorkerId = agentId.ToString(),
            StartedAt = now.AddMinutes(-5),
            CreatedAt = now.AddMinutes(-6),
        });
        await db.SaveChangesAsync();
        return (agentId, apiKey, runId);
    }

    private async Task<WebSocket> ConnectAgentAsync(string apiKey)
    {
        var client = _fx.Server.CreateWebSocketClient();
        var socket = await client.ConnectAsync(
            new Uri($"ws://localhost/ws/agent?key={Uri.EscapeDataString(apiKey)}"),
            CancellationToken.None);
        var buffer = new byte[16 * 1024];
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(15));
        var received = await socket.ReceiveAsync(buffer, cts.Token);
        using var doc = JsonDocument.Parse(Encoding.UTF8.GetString(buffer, 0, received.Count));
        Assert.Equal("welcome", doc.RootElement.GetProperty("type").GetString());
        return socket;
    }

    private static readonly string[] ExpectedCaseIds =
        ["api-aggregate", "api-compress", "api-search", "api-transform", "api-users"];

    [Fact]
    public async Task Apibench_artifact_cases_survive_agent_to_api_end_to_end()
    {
        var (_, apiKey, runId) = await SeedAgentWithRunningRunAsync();

        // 1. The real agent executor produces the terminal frame.
        var finished = await ExecuteApibenchRunAsync(runId);

        // 2. Glue contract: the artifact's summaries must be parseable by the
        // control plane's regression analyzer — the old {success,failure}
        // placeholder parsed to [] and silently disabled detection.
        var stats = RegressionAnalyzer.ParseSummaries(finished.Artifact!.Summaries.GetRawText());
        Assert.Equal(ExpectedCaseIds, stats.Select(s => s.CaseId).Order(StringComparer.Ordinal));
        Assert.All(stats, s =>
        {
            Assert.Equal(15.0, s.P50); // median of the fixture's 10ms/20ms
            Assert.Equal(2, s.SuccessCount);
            Assert.Equal(1, s.FailureCount);
            Assert.Equal(2, s.IncludedSampleCount);
        });

        // 3. Ingest the frame over the real socket, serialized by the agent's
        // own wire encoder (no hand-rolled frame that could mask drift).
        using var socket = await ConnectAgentAsync(apiKey);
        var wire = JsonSerializer.Serialize<AgentMessage>(finished);
        await socket.SendAsync(
            Encoding.UTF8.GetBytes(wire), WebSocketMessageType.Text, true, CancellationToken.None);

        var deadline = DateTime.UtcNow + TimeSpan.FromSeconds(10);
        string? status = null;
        while (DateTime.UtcNow < deadline)
        {
            await using var db = _fx.NewDbContext();
            status = await db.TestRuns.AsNoTracking()
                .Where(r => r.Id == runId).Select(r => r.Status).FirstAsync();
            if (status == "completed")
                break;
            await Task.Delay(200);
        }
        Assert.Equal("completed", status);

        // 4. The artifact endpoint of issue #796 must now serve the per-case
        // results.
        using var http = _fx.CreateAuthenticatedClient();
        var resp = await http.GetAsync($"/api/v2/test-runs/{runId}/artifact");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);

        using var artifact = JsonDocument.Parse(await resp.Content.ReadAsStringAsync());
        var root = artifact.RootElement;

        var caseIds = root.GetProperty("cases").EnumerateArray()
            .Select(c => c.GetProperty("id").GetString()).ToArray();
        Assert.Equal(ExpectedCaseIds, caseIds);

        var summaries = root.GetProperty("summaries");
        Assert.Equal(JsonValueKind.Array, summaries.ValueKind);
        Assert.Equal(ExpectedCaseIds.Length, summaries.GetArrayLength());
        foreach (var s in summaries.EnumerateArray())
        {
            Assert.Contains(s.GetProperty("case_id").GetString(), ExpectedCaseIds);
            Assert.Equal(15.0, s.GetProperty("p50").GetDouble());
            Assert.Equal(2, s.GetProperty("success_count").GetInt64());
        }
    }
}
