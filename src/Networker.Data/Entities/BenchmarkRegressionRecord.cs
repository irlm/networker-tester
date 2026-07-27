using System;

namespace Networker.Data.Entities;

/// <summary>
/// One detected benchmark regression (V047 <c>benchmark_regression</c>): a
/// completed run whose per-case p50 worsened by more than 10% vs the baseline
/// run, or whose per-case success rate dropped below 99%. Written by
/// <c>BenchmarkRegressionDetector</c> on run completion; served by
/// <c>GET /api/projects/{projectId}/benchmark-regressions</c>.
/// </summary>
public partial class BenchmarkRegressionRecord
{
    public Guid RegressionId { get; set; }

    public Guid TestConfigId { get; set; }

    public Guid TestRunId { get; set; }

    /// <summary>The run compared against; SET NULL when that run is pruned.</summary>
    public Guid? BaselineRunId { get; set; }

    public string CaseId { get; set; } = null!;

    /// <summary>Which policy check fired: <c>p50</c>-family or <c>success_rate</c>.</summary>
    public string Metric { get; set; } = null!;

    /// <summary>Unit of the metric values (<c>ms</c>, <c>MB/s</c>, <c>%</c>, …).</summary>
    public string MetricUnit { get; set; } = null!;

    public double BaselineValue { get; set; }

    public double CurrentValue { get; set; }

    /// <summary>
    /// Relative change of p50 vs baseline in percent; for
    /// <c>success_rate</c> the change in percentage points.
    /// </summary>
    public double DeltaPercent { get; set; }

    public string Severity { get; set; } = null!;

    public DateTime DetectedAt { get; set; }

    public virtual TestConfig TestConfig { get; set; } = null!;

    public virtual TestRun TestRun { get; set; } = null!;

    public virtual TestRun? BaselineRun { get; set; }
}
