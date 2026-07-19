using Npgsql;

namespace Networker.ControlPlane.Alerting;

/// <summary>
/// Extracts a rule metric for one run.
///
/// <para>Rate metrics (<c>success_rate</c> / <c>error_rate</c>) come from the
/// control-plane <c>test_run</c> counters the agent streams during the run —
/// no extra I/O. Latency metrics (<c>p95_ms</c> / <c>mean_ms</c>) come from
/// the probe-result tables the networker-tester engine persists
/// (<c>RequestAttempt</c> / <c>HttpResult</c>, V001 tester-owned schema — NOT
/// in the EF model, so raw Npgsql like <c>UrlTestsEndpoints</c>): per
/// successful attempt, <c>HttpResult.TotalDurationMs</c> when present, else
/// the attempt's wall time. A missing tester schema (42P01, e.g. no probe has
/// ever written results) yields null, which the evaluation layer treats as
/// "no data" — never a breach.</para>
///
/// <para>Scoped; caches the per-run latency aggregate so a run evaluated
/// against many rules pays the SQL once.</para>
/// </summary>
public sealed class RunMetricProvider(NpgsqlDataSource dataSource)
{
    private readonly Dictionary<Guid, (double? MeanMs, double? P95Ms)> _latencyCache = new();

    /// <summary>
    /// Resolve <paramref name="metric"/> for the run; null = not measurable
    /// (no attempts recorded / tester schema absent / unknown metric).
    /// </summary>
    public async Task<double?> GetAsync(
        Guid runId, int successCount, int failureCount, string metric, CancellationToken ct = default)
    {
        switch (metric)
        {
            case AlertRuleLogic.MetricSuccessRate:
                return AlertRuleLogic.SuccessRate(successCount, failureCount);
            case AlertRuleLogic.MetricErrorRate:
                return AlertRuleLogic.ErrorRate(successCount, failureCount);
            case AlertRuleLogic.MetricMeanMs:
                return (await GetLatencyAsync(runId, ct)).MeanMs;
            case AlertRuleLogic.MetricP95Ms:
                return (await GetLatencyAsync(runId, ct)).P95Ms;
            default:
                return null;
        }
    }

    private async Task<(double? MeanMs, double? P95Ms)> GetLatencyAsync(Guid runId, CancellationToken ct)
    {
        if (_latencyCache.TryGetValue(runId, out var cached))
        {
            return cached;
        }

        // Per-attempt latency of SUCCESSFUL attempts: the HTTP total duration
        // when an HttpResult exists (http1/2/3 modes), else the attempt's
        // started→finished wall time (dns/tcp/tls/udp modes). Unquoted
        // identifiers fold to lowercase on both sides, matching how the tester
        // creates these tables.
        const string sql = """
            SELECT AVG(v.val),
                   PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY v.val)
            FROM (
                SELECT COALESCE(h.TotalDurationMs,
                                EXTRACT(EPOCH FROM (a.FinishedAt - a.StartedAt)) * 1000.0) AS val
                FROM RequestAttempt a
                LEFT JOIN HttpResult h ON h.AttemptId = a.AttemptId
                WHERE a.RunId = $1 AND a.Success
            ) v
            WHERE v.val IS NOT NULL
            """;

        (double? MeanMs, double? P95Ms) result = (null, null);
        try
        {
            await using var cmd = dataSource.CreateCommand(sql);
            cmd.Parameters.AddWithValue(runId);
            await using var reader = await cmd.ExecuteReaderAsync(ct);
            if (await reader.ReadAsync(ct))
            {
                result = (
                    reader.IsDBNull(0) ? null : reader.GetDouble(0),
                    reader.IsDBNull(1) ? null : reader.GetDouble(1));
            }
        }
        catch (PostgresException ex) when (ex.SqlState == PostgresErrorCodes.UndefinedTable)
        {
            // Tester result schema not present in this database — no data.
        }

        _latencyCache[runId] = result;
        return result;
    }
}
