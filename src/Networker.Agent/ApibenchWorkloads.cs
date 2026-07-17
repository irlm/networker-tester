using System.Text.Json;

namespace Networker.Agent;

/// <summary>
/// The apibench workload set — the measured <c>/api/*</c> compute endpoints
/// from <c>benchmarks/shared/API-SPEC.md</c> §4 (audit C1: nothing in the
/// product measured these before wave 3).
///
/// The definitions are the committed <c>benchmarks/configs/apibench.json</c>,
/// embedded into this assembly at build time (see the csproj) so agent,
/// orchestrator, and weekly CI all drive byte-identical request shapes.
///
/// "apibench" is a runner-level mode, not a tester protocol: the tester is
/// invoked once per workload over HTTP/1.1 with the workload's path/query and
/// (for POST) <c>--request-body</c> / <c>--request-content-type</c>.
/// </summary>
internal static class ApibenchWorkloads
{
    public const string ModeName = "apibench";

    /// <summary>One measured API workload (mirrors the orchestrator's
    /// <c>workloads::ApiWorkload</c>).</summary>
    public sealed record Workload(
        string Name,
        string Method,
        string Path,
        string? Body,
        string? ContentType);

    private static readonly Lazy<IReadOnlyList<Workload>> LazyAll = new(Load);

    public static IReadOnlyList<Workload> All => LazyAll.Value;

    public static bool IsApibenchMode(string mode) =>
        string.Equals(mode, ModeName, StringComparison.OrdinalIgnoreCase);

    private static IReadOnlyList<Workload> Load()
    {
        using var stream = typeof(ApibenchWorkloads).Assembly
            .GetManifestResourceStream("Networker.Agent.apibench.json")
            ?? throw new InvalidOperationException("embedded apibench.json missing");
        using var doc = JsonDocument.Parse(stream);

        var result = new List<Workload>();
        foreach (var w in doc.RootElement.GetProperty("workloads").EnumerateArray())
        {
            result.Add(new Workload(
                Name: w.GetProperty("name").GetString()!,
                Method: w.GetProperty("method").GetString()!,
                Path: w.GetProperty("path").GetString()!,
                Body: w.TryGetProperty("body", out var b) ? b.GetString() : null,
                ContentType: w.TryGetProperty("content_type", out var ct) ? ct.GetString() : null));
        }

        if (result.Count == 0)
            throw new InvalidOperationException("embedded apibench.json contains no workloads");
        return result;
    }

    /// <summary>Rewrite a resolved target URL (e.g. <c>https://host:8443/health</c>)
    /// to the workload's path + query, preserving scheme/host/port.</summary>
    public static string WorkloadTarget(string baseTarget, Workload workload)
    {
        // GetLeftPart(Authority) yields scheme://host[:port] (non-default
        // ports preserved, no trailing slash) — append the workload path+query.
        var uri = new Uri(baseTarget);
        return uri.GetLeftPart(UriPartial.Authority) + workload.Path;
    }

    /// <summary>Tester CLI args for one workload. Same measurement pipeline as
    /// a normal run (<c>--json-stdout</c>); only the request shape differs.</summary>
    public static List<string> BuildArgs(TestConfigView config, string baseTarget, Workload workload)
    {
        var timeoutSecs = Math.Max(1u, (config.TimeoutMs + 999) / 1000);
        var args = new List<string>
        {
            "--target", WorkloadTarget(baseTarget, workload),
            "--modes", "http1",
            "--runs", config.Runs.ToString(),
            "--concurrency", config.Concurrency.ToString(),
            "--timeout", timeoutSecs.ToString(),
            "--json-stdout",
        };
        if (config.Insecure)
            args.Add("--insecure");
        if (workload.Method == "POST" && !string.IsNullOrEmpty(workload.Body))
        {
            args.Add("--request-body");
            args.Add(workload.Body);
            args.Add("--request-content-type");
            args.Add(workload.ContentType ?? "application/json");
        }
        return args;
    }
}
