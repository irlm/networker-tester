using System.Globalization;
using System.Text.Json;

namespace Networker.Agent;

/// <summary>
/// Synthesizes the per-case sections of the agent's <c>BenchmarkArtifact</c>
/// (<c>cases</c> + <c>summaries</c> + <c>data_quality</c>) from the attempts
/// the executor already observes — streamed live or parsed from the tester's
/// final TestRun JSON.
///
/// <para>Until #796 the agent's artifact was a placeholder: <c>cases: []</c>
/// and <c>summaries: {"success":N,"failure":N}</c>. That made the per-case
/// apibench results (api-users / api-transform / api-aggregate / api-search /
/// api-compress p50/p95/success) unretrievable from
/// <c>GET /api/v2/test-runs/{id}/artifact</c>, and — because
/// <c>RegressionAnalyzer.ParseSummaries</c> expects a per-case ARRAY — silently
/// disabled benchmark regression detection for every agent-executed run.</para>
///
/// <para><b>Case identity</b> (stable across runs so the regression detector
/// can match baselines):</para>
/// <list type="bullet">
///   <item>apibench workload invocations → the workload name
///   (<c>api-users</c>, …) — one case per measured <c>/api/*</c> endpoint.</item>
///   <item>base protocol invocations → <c>{protocol}:{payload}:{stack}</c>,
///   mirroring the tester's <c>benchmark_case_id</c>
///   (<c>crates/networker-tester/src/output/json.rs</c>).</item>
/// </list>
///
/// <para><b>Summary shape</b>: the field subset of the tester's
/// <c>BenchmarkSummary</c> that downstream consumers read —
/// <c>RegressionAnalyzer.ParseSummaries</c> (case_id, metric_unit,
/// higher_is_better, p50, success_count, failure_count,
/// included_sample_count, ci95_*) and the dashboard's ArtifactSection
/// (protocol, metric_name, included_sample_count, p50/p95/p99, rps, stddev).
/// The percentile ladder is linear-interpolation over the included (successful,
/// metric-bearing) samples; ci95 is the rank-based (order-statistic) 95% CI of
/// the median, emitted only at n ≥ 10 — below that the all-zero placeholder
/// keeps the analyzer on its pure-threshold path.</para>
/// </summary>
internal sealed class BenchmarkArtifactBuilder
{
    /// <summary>Minimum included samples for a case to count as adequately
    /// sampled AND for a rank-based median CI to be emitted (matches the
    /// analyzer's small-n guard, RegressionAnalyzer.MinSamples).</summary>
    internal const int MinSamplesForCi = 10;

    private sealed class CaseAccumulator
    {
        public required string CaseId { get; init; }
        public required string Protocol { get; init; }
        public string? HttpStack { get; set; }
        public long? PayloadBytes { get; set; }
        public required string MetricName { get; init; }
        public required string MetricUnit { get; init; }
        public bool HigherIsBetter { get; init; }
        public long SuccessCount { get; set; }
        public long FailureCount { get; set; }
        public List<double> Values { get; } = [];
        public DateTime? FirstStartedAt { get; set; }
        public DateTime? LastFinishedAt { get; set; }
    }

    private readonly Dictionary<string, CaseAccumulator> _cases = new(StringComparer.Ordinal);

    public bool HasCases => _cases.Count > 0;

    /// <summary>
    /// Record one attempt (the tester's serialized <c>RequestAttempt</c>).
    /// <paramref name="workload"/> is the apibench workload name for workload
    /// invocations, null for base protocol invocations.
    /// </summary>
    public void Record(string? workload, JsonElement attempt)
    {
        if (attempt.ValueKind != JsonValueKind.Object)
            return;

        var protocol = GetString(attempt, "protocol") ?? "unknown";
        var success = attempt.TryGetProperty("success", out var s) && s.ValueKind == JsonValueKind.True;
        var (metricName, metricUnit, higherIsBetter, metricValue) = PrimaryMetric(attempt, protocol);
        var httpStack = GetString(attempt, "http_stack");
        var payloadBytes = AttemptPayloadBytes(attempt);

        var caseId = workload ?? string.Create(CultureInfo.InvariantCulture,
            $"{protocol}:{(payloadBytes is { } pb ? pb.ToString(CultureInfo.InvariantCulture) : "default")}:{(httpStack ?? "default").Replace(':', '_')}");

        if (!_cases.TryGetValue(caseId, out var acc))
        {
            acc = new CaseAccumulator
            {
                CaseId = caseId,
                Protocol = protocol,
                MetricName = metricName,
                MetricUnit = metricUnit,
                HigherIsBetter = higherIsBetter,
                HttpStack = httpStack,
                PayloadBytes = payloadBytes,
            };
            _cases.Add(caseId, acc);
        }

        if (success)
            acc.SuccessCount++;
        else
            acc.FailureCount++;

        // Included samples: successful attempts that produced a finite metric
        // (mirrors the tester's inclusion_status: failures and metric-less
        // attempts are excluded from the distribution but still counted).
        if (success && metricValue is { } v && double.IsFinite(v))
            acc.Values.Add(v);

        if (GetDateTime(attempt, "started_at") is { } startedAt
            && (acc.FirstStartedAt is null || startedAt < acc.FirstStartedAt))
        {
            acc.FirstStartedAt = startedAt;
        }
        if (GetDateTime(attempt, "finished_at") is { } finishedAt
            && (acc.LastFinishedAt is null || finishedAt > acc.LastFinishedAt))
        {
            acc.LastFinishedAt = finishedAt;
        }
    }

    /// <summary>The artifact's <c>cases</c> array (tester BenchmarkCase shape).</summary>
    public string BuildCasesJson()
    {
        using var stream = new MemoryStream();
        using (var w = new Utf8JsonWriter(stream))
        {
            w.WriteStartArray();
            foreach (var c in Ordered())
            {
                w.WriteStartObject();
                w.WriteString("id", c.CaseId);
                w.WriteString("protocol", c.Protocol);
                if (c.PayloadBytes is { } pb)
                    w.WriteNumber("payload_bytes", pb);
                if (c.HttpStack is { } stack)
                    w.WriteString("http_stack", stack);
                w.WriteString("metric_name", c.MetricName);
                w.WriteString("metric_unit", c.MetricUnit);
                w.WriteBoolean("higher_is_better", c.HigherIsBetter);
                w.WriteEndObject();
            }
            w.WriteEndArray();
        }
        return System.Text.Encoding.UTF8.GetString(stream.ToArray());
    }

    /// <summary>The artifact's <c>summaries</c> array — one per-case stats
    /// object in the tester's BenchmarkSummary field subset.</summary>
    public string BuildSummariesJson()
    {
        using var stream = new MemoryStream();
        using (var w = new Utf8JsonWriter(stream))
        {
            w.WriteStartArray();
            foreach (var c in Ordered())
                WriteSummary(w, c);
            w.WriteEndArray();
        }
        return System.Text.Encoding.UTF8.GetString(stream.ToArray());
    }

    /// <summary>The artifact's <c>data_quality</c> section, in the tester's
    /// vocabulary (sufficiency: adequate/marginal/insufficient) and honest
    /// about what agent-side synthesis cannot claim.</summary>
    public string BuildDataQualityJson()
    {
        var included = _cases.Values.Sum(c => (long)c.Values.Count);
        var sufficiency = _cases.Count > 0 && _cases.Values.All(c => c.Values.Count >= MinSamplesForCi)
            ? "adequate"
            : included > 0 ? "marginal" : "insufficient";

        // Aggregate stability: the worst per-case coefficient of variation.
        var worstCv = 0.0;
        foreach (var c in _cases.Values)
        {
            if (c.Values.Count < 2)
                continue;
            var mean = c.Values.Average();
            if (mean <= 0.0)
                continue;
            var cv = Stddev(c.Values, mean) / mean * 100.0;
            if (cv > worstCv)
                worstCv = cv;
        }

        using var stream = new MemoryStream();
        using (var w = new Utf8JsonWriter(stream))
        {
            w.WriteStartObject();
            w.WriteString("noise_level", "unknown");
            w.WriteNumber("sample_stability_cv", Round(worstCv));
            w.WriteString("sufficiency", sufficiency);
            w.WriteStartArray("warnings");
            w.WriteEndArray();
            w.WriteBoolean("publication_ready", false);
            w.WriteStartArray("publication_blockers");
            w.WriteStringValue(
                "agent-side artifact: statistics derived from live attempt measurements without "
                + "the tester's controlled benchmark phases (warmup/pilot/stability-check)");
            w.WriteEndArray();
            w.WriteEndObject();
        }
        return System.Text.Encoding.UTF8.GetString(stream.ToArray());
    }

    private IEnumerable<CaseAccumulator> Ordered() =>
        _cases.Values.OrderBy(c => c.CaseId, StringComparer.Ordinal);

    private static void WriteSummary(Utf8JsonWriter w, CaseAccumulator c)
    {
        var values = c.Values.Order().ToList();
        var n = values.Count;
        var total = c.SuccessCount + c.FailureCount;
        var mean = n > 0 ? values.Average() : 0.0;
        var stddev = n > 1 ? Stddev(values, mean) : 0.0;

        // Rank-based 95% CI of the median (order statistics, normal
        // approximation of the binomial ranks). Only at n >= MinSamplesForCi;
        // below that the 0.0/0.0 placeholder keeps the regression analyzer on
        // its pure-threshold path (it treats all-zero CIs as unusable).
        double ci95Lower = 0.0, ci95Upper = 0.0;
        if (n >= MinSamplesForCi)
        {
            var half = 1.96 * Math.Sqrt(n) / 2.0;
            var lo = (int)Math.Clamp(Math.Floor(n / 2.0 - half), 0, n - 1);
            var hi = (int)Math.Clamp(Math.Ceiling(n / 2.0 + half), 0, n - 1);
            ci95Lower = values[lo];
            ci95Upper = values[hi];
        }

        // Wall-clock rate over the case's active window; 0 when timestamps
        // are missing or degenerate (never a fabricated rate).
        var rps = 0.0;
        if (c.FirstStartedAt is { } first && c.LastFinishedAt is { } last && last > first)
            rps = c.SuccessCount / (last - first).TotalSeconds;

        w.WriteStartObject();
        w.WriteString("case_id", c.CaseId);
        w.WriteString("protocol", c.Protocol);
        if (c.PayloadBytes is { } pb)
            w.WriteNumber("payload_bytes", pb);
        if (c.HttpStack is { } stack)
            w.WriteString("http_stack", stack);
        w.WriteString("metric_name", c.MetricName);
        w.WriteString("metric_unit", c.MetricUnit);
        w.WriteBoolean("higher_is_better", c.HigherIsBetter);
        w.WriteNumber("sample_count", total);
        w.WriteNumber("included_sample_count", n);
        w.WriteNumber("excluded_sample_count", total - n);
        w.WriteNumber("success_count", c.SuccessCount);
        w.WriteNumber("failure_count", c.FailureCount);
        w.WriteNumber("total_requests", total);
        w.WriteNumber("error_count", c.FailureCount);
        w.WriteNumber("rps", Round(rps));
        w.WriteNumber("min", Round(n > 0 ? values[0] : 0.0));
        w.WriteNumber("mean", Round(mean));
        w.WriteNumber("p5", Round(Percentile(values, 0.05)));
        w.WriteNumber("p25", Round(Percentile(values, 0.25)));
        w.WriteNumber("p50", Round(Percentile(values, 0.50)));
        w.WriteNumber("p75", Round(Percentile(values, 0.75)));
        w.WriteNumber("p95", Round(Percentile(values, 0.95)));
        w.WriteNumber("p99", Round(Percentile(values, 0.99)));
        w.WriteNumber("p999", Round(Percentile(values, 0.999)));
        w.WriteNumber("max", Round(n > 0 ? values[n - 1] : 0.0));
        w.WriteNumber("stddev", Round(stddev));
        w.WriteNumber("ci95_lower", Round(ci95Lower));
        w.WriteNumber("ci95_upper", Round(ci95Upper));
        w.WriteEndObject();
    }

    /// <summary>Linear-interpolation percentile over a SORTED list; 0 when empty.</summary>
    internal static double Percentile(IReadOnlyList<double> sorted, double q)
    {
        if (sorted.Count == 0)
            return 0.0;
        if (sorted.Count == 1)
            return sorted[0];
        var pos = (sorted.Count - 1) * q;
        var lo = (int)Math.Floor(pos);
        var hi = (int)Math.Ceiling(pos);
        if (lo == hi)
            return sorted[lo];
        return sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo);
    }

    private static double Stddev(IReadOnlyList<double> values, double mean)
    {
        if (values.Count < 2)
            return 0.0;
        var sumSq = values.Sum(v => (v - mean) * (v - mean));
        return Math.Sqrt(sumSq / (values.Count - 1));
    }

    /// <summary>Round for the wire — 4 decimals is sub-microsecond for ms
    /// metrics and keeps the JSON free of 17-digit float noise.</summary>
    private static double Round(double v) =>
        double.IsFinite(v) ? Math.Round(v, 4) : 0.0;

    // ── Primary metric per protocol (mirrors the tester's primary_metric_value /
    // primary_metric_label / metric_unit_for_protocol in metrics.rs for the
    // protocol families the agent dispatches; protocols without a mapped
    // metric contribute success/failure counts only) ─────────────────────────
    private static (string Name, string Unit, bool HigherIsBetter, double? Value) PrimaryMetric(
        JsonElement attempt, string protocol)
    {
        switch (protocol)
        {
            case "http1" or "http2" or "http3" or "native" or "curl":
                return ("Total ms", "ms", false, GetNumber(attempt, "http", "total_duration_ms"));
            case "download" or "download1" or "download2" or "download3"
                or "upload" or "upload1" or "upload2" or "upload3"
                or "webdownload" or "webupload":
                return ("Throughput MB/s", "MB/s", true, GetNumber(attempt, "http", "throughput_mbps"));
            case "udpdownload" or "udpupload":
                return ("Throughput MB/s", "MB/s", true, GetNumber(attempt, "udp_throughput", "throughput_mbps"));
            case "tcp":
                return ("Connect ms", "ms", false, GetNumber(attempt, "tcp", "connect_duration_ms"));
            case "dns":
                return ("Resolve ms", "ms", false, GetNumber(attempt, "dns", "duration_ms"));
            case "tls" or "tlsresume":
                return ("Handshake ms", "ms", false, GetNumber(attempt, "tls", "handshake_duration_ms"));
            case "udp":
                // A fully-lost attempt's rtt_avg is a 0.0 sentinel, not a
                // measurement (trust audit V11) — same guard as the tester.
                var udpSuccesses = GetNumber(attempt, "udp", "success_count");
                return ("RTT avg ms", "ms", false,
                    udpSuccesses is > 0 ? GetNumber(attempt, "udp", "rtt_avg_ms") : null);
            case "ping":
                var pingSuccesses = GetNumber(attempt, "ping", "success_count");
                return ("RTT avg ms", "ms", false,
                    pingSuccesses is > 0 ? GetNumber(attempt, "ping", "rtt_avg_ms") : null);
            case "pageload" or "pageload2" or "pageload3":
                return ("Total ms", "ms", false, GetNumber(attempt, "page_load", "total_ms"));
            default:
                return ("Total ms", "ms", false, null);
        }
    }

    /// <summary>Mirror of the tester's <c>attempt_payload_bytes</c>: the HTTP
    /// payload when &gt; 0, else the UDP-throughput payload when &gt; 0.</summary>
    private static long? AttemptPayloadBytes(JsonElement attempt)
    {
        var http = GetNumber(attempt, "http", "payload_bytes");
        if (http is > 0)
            return (long)http.Value;
        var udp = GetNumber(attempt, "udp_throughput", "payload_bytes");
        return udp is > 0 ? (long)udp.Value : null;
    }

    private static string? GetString(JsonElement el, string name) =>
        el.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String
            ? v.GetString()
            : null;

    private static double? GetNumber(JsonElement el, string section, string name) =>
        el.TryGetProperty(section, out var sec)
            && sec.ValueKind == JsonValueKind.Object
            && sec.TryGetProperty(name, out var v)
            && v.ValueKind == JsonValueKind.Number
            ? v.GetDouble()
            : null;

    private static DateTime? GetDateTime(JsonElement el, string name) =>
        el.TryGetProperty(name, out var v)
            && v.ValueKind == JsonValueKind.String
            && v.TryGetDateTime(out var dt)
            ? dt
            : null;
}
