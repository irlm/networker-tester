using System.Runtime.InteropServices;
using System.Text.Json;
using Microsoft.Extensions.Logging.Abstractions;
using Networker.Agent;

namespace Networker.Agent.Tests;

/// <summary>
/// Tester-output → emitted-frames mapping. Drives the real
/// <see cref="RunExecutor"/> against a fake <c>networker-tester</c> (a tiny
/// script that prints canned JSON), and asserts the exact frame sequence +
/// terminal <c>run_finished</c> status the Rust executor would produce.
/// </summary>
public class RunExecutorMappingTests : IDisposable
{
    private readonly string _scratch = Directory.CreateTempSubdirectory("agent-tests").FullName;

    public void Dispose()
    {
        try { Directory.Delete(_scratch, recursive: true); } catch { /* best-effort */ }
    }

    // Collect every frame the executor emits so we can assert the sequence.
    private sealed class CollectingSink : RawWebSocketClient.IFrameSink
    {
        public List<AgentMessage> Messages { get; } = [];
        public bool TrySend(AgentMessage message)
        {
            Messages.Add(message);
            return true;
        }
    }

    /// <summary>Write a fake tester that echoes <paramref name="stdout"/> and
    /// exits with <paramref name="exitCode"/>. Cross-platform (sh / cmd).</summary>
    private string WriteFakeTester(string stdout, int exitCode = 0, string? stderr = null)
    {
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
        {
            var path = Path.Combine(_scratch, "networker-tester.cmd");
            // Encode the JSON as a here-ish echo. Keep it single-line to dodge cmd quirks.
            var lines = new List<string> { "@echo off" };
            foreach (var line in stdout.Split('\n'))
                lines.Add($"echo {line.Replace("%", "%%").Replace("^", "^^").Replace("&", "^&").Replace("<", "^<").Replace(">", "^>").Replace("|", "^|")}");
            if (stderr is not null)
                lines.Add($"echo {stderr} 1>&2");
            lines.Add($"exit /b {exitCode}");
            File.WriteAllText(path, string.Join("\r\n", lines));
            return path;
        }
        else
        {
            var path = Path.Combine(_scratch, "networker-tester");
            var script = "#!/bin/sh\ncat <<'NWEOF'\n" + stdout + "\nNWEOF\n";
            if (stderr is not null)
                script += $"echo '{stderr}' 1>&2\n";
            script += $"exit {exitCode}\n";
            File.WriteAllText(path, script);
            // chmod +x
            var psi = new System.Diagnostics.ProcessStartInfo("chmod", $"+x \"{path}\"") { UseShellExecute = false };
            System.Diagnostics.Process.Start(psi)!.WaitForExit();
            return path;
        }
    }

    private RunExecutor MakeExecutor(string testerPath) =>
        new(NullLogger<RunExecutor>.Instance, new AgentOptions { TesterPath = testerPath });

    private static JsonElement NetworkConfig(string? methodology = null)
    {
        var meth = methodology is null ? "" : $""", "methodology": {methodology}""";
        return JsonDocument.Parse($$"""
            { "id":"33333333-3333-3333-3333-333333333333",
              "endpoint": { "kind":"network", "host":"example.com" },
              "workload": { "modes":["http1"], "runs":2, "concurrency":1, "timeout_ms":3000,
                            "payload_sizes":[], "capture_mode":"headers-only", "insecure":false }{{meth}} }
            """).RootElement.Clone();
    }

    [Fact]
    public async Task Successful_run_emits_started_attempts_progress_and_completed()
    {
        var testerJson = """
            {"schema_version":"1.0","run_id":"r","target_url":"https://example.com/health",
             "attempts":[{"attempt_id":"a1","protocol":"http1","success":true},
                         {"attempt_id":"a2","protocol":"http1","success":false}]}
            """;
        var exec = MakeExecutor(WriteFakeTester(testerJson));
        var sink = new CollectingSink();
        var runId = Guid.NewGuid();

        await exec.ExecuteAsync(runId, NetworkConfig(), sink, CancellationToken.None);

        // First frame: run_started.
        Assert.IsType<RunStartedMessage>(sink.Messages[0]);

        // Two attempt_event frames, one per attempt, forwarded verbatim.
        var attempts = sink.Messages.OfType<AttemptEventMessage>().ToList();
        Assert.Equal(2, attempts.Count);
        Assert.Equal("a1", attempts[0].Attempt.GetProperty("attempt_id").GetString());
        Assert.Equal("a2", attempts[1].Attempt.GetProperty("attempt_id").GetString());

        // A final run_progress with the success/failure tallies.
        var progress = sink.Messages.OfType<RunProgressMessage>().Last();
        Assert.Equal(1u, progress.Success);
        Assert.Equal(1u, progress.Failure);

        // Terminal run_finished = completed, no artifact (not benchmark).
        var finished = Assert.IsType<RunFinishedMessage>(sink.Messages[^1]);
        Assert.Equal("completed", finished.Status);
        Assert.Null(finished.Artifact);
    }

    [Fact]
    public async Task Nonzero_exit_but_parseable_json_still_completed()
    {
        var testerJson = """{"schema_version":"1.0","attempts":[{"attempt_id":"a1","success":true}]}""";
        var exec = MakeExecutor(WriteFakeTester(testerJson, exitCode: 1));
        var sink = new CollectingSink();

        await exec.ExecuteAsync(Guid.NewGuid(), NetworkConfig(), sink, CancellationToken.None);

        var finished = Assert.IsType<RunFinishedMessage>(sink.Messages[^1]);
        Assert.Equal("completed", finished.Status);
    }

    [Fact]
    public async Task Unparseable_output_maps_to_failed_with_error_frame()
    {
        var exec = MakeExecutor(WriteFakeTester("this is not json", exitCode: 1));
        var sink = new CollectingSink();

        await exec.ExecuteAsync(Guid.NewGuid(), NetworkConfig(), sink, CancellationToken.None);

        Assert.Contains(sink.Messages, m => m is ErrorMessage e && e.Message.Contains("unparseable JSON"));
        var finished = Assert.IsType<RunFinishedMessage>(sink.Messages[^1]);
        Assert.Equal("failed", finished.Status);
    }

    [Fact]
    public async Task Benchmark_config_synthesizes_artifact_on_completion()
    {
        var testerJson = """{"schema_version":"1.0","attempts":[{"attempt_id":"a1","success":true}]}""";
        var meth = """{ "warmup_runs":5, "measured_runs":30, "cooldown_ms":100 }""";
        var exec = MakeExecutor(WriteFakeTester(testerJson));
        var sink = new CollectingSink();

        await exec.ExecuteAsync(Guid.NewGuid(), NetworkConfig(meth), sink, CancellationToken.None);

        var finished = Assert.IsType<RunFinishedMessage>(sink.Messages[^1]);
        Assert.Equal("completed", finished.Status);
        Assert.NotNull(finished.Artifact);
        Assert.Equal(1, finished.Artifact!.Summaries.GetProperty("success").GetInt32());
        Assert.Equal(0, finished.Artifact.Summaries.GetProperty("failure").GetInt32());
    }

    [Fact]
    public async Task Unsupported_endpoint_kind_maps_to_error_and_failed()
    {
        var exec = MakeExecutor(WriteFakeTester("{}")); // tester never runs
        var sink = new CollectingSink();
        var proxyConfig = JsonDocument.Parse("""
            { "id":"33333333-3333-3333-3333-333333333333",
              "endpoint": { "kind":"proxy", "proxy_endpoint_id":"00000000-0000-0000-0000-000000000001" },
              "workload": { "modes":["http1"], "runs":1, "concurrency":1, "timeout_ms":3000,
                            "payload_sizes":[], "capture_mode":"headers-only", "insecure":false } }
            """).RootElement.Clone();

        await exec.ExecuteAsync(Guid.NewGuid(), proxyConfig, sink, CancellationToken.None);

        Assert.Contains(sink.Messages, m => m is ErrorMessage e && e.Message.Contains("Unsupported endpoint kind"));
        var finished = Assert.IsType<RunFinishedMessage>(sink.Messages[^1]);
        Assert.Equal("failed", finished.Status);
    }

    [Fact]
    public async Task Apibench_only_config_runs_one_tester_invocation_per_workload()
    {
        // modes=["apibench"] → no base invocation, one tester process per
        // workload (5). The fake tester emits one successful attempt per
        // invocation, so we expect 5 attempt frames and completed.
        var testerJson = """{"schema_version":"1.0","attempts":[{"attempt_id":"a1","protocol":"http1","success":true}]}""";
        var exec = MakeExecutor(WriteFakeTester(testerJson));
        var sink = new CollectingSink();
        var apibenchConfig = JsonDocument.Parse("""
            { "id":"44444444-4444-4444-4444-444444444444",
              "endpoint": { "kind":"network", "host":"example.com", "port":8443 },
              "workload": { "modes":["apibench"], "runs":1, "concurrency":1, "timeout_ms":3000,
                            "payload_sizes":[], "capture_mode":"headers-only", "insecure":true } }
            """).RootElement.Clone();

        await exec.ExecuteAsync(Guid.NewGuid(), apibenchConfig, sink, CancellationToken.None);

        var attempts = sink.Messages.OfType<AttemptEventMessage>().Count();
        Assert.Equal(ApibenchWorkloads.All.Count, attempts);

        var progress = sink.Messages.OfType<RunProgressMessage>().Last();
        Assert.Equal((uint)ApibenchWorkloads.All.Count, progress.Success);
        Assert.Equal(0u, progress.Failure);

        var finished = Assert.IsType<RunFinishedMessage>(sink.Messages[^1]);
        Assert.Equal("completed", finished.Status);
    }

    [Fact]
    public async Task Missing_tester_binary_maps_to_error_and_failed()
    {
        var exec = MakeExecutor(Path.Combine(_scratch, "does-not-exist"));
        var sink = new CollectingSink();

        await exec.ExecuteAsync(Guid.NewGuid(), NetworkConfig(), sink, CancellationToken.None);

        Assert.Contains(sink.Messages, m => m is ErrorMessage);
        var finished = Assert.IsType<RunFinishedMessage>(sink.Messages[^1]);
        Assert.Equal("failed", finished.Status);
    }
}
