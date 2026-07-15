using System.Text.Json;
using Networker.ControlPlane.Realtime;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// One detected metric regression — the C# port of Rust <c>Regression</c> in
/// <c>crates/networker-dashboard/src/regression.rs</c>.
/// </summary>
public sealed record Regression(
    string Phase,
    string Metric,
    double BaselineValue,
    double CurrentValue,
    double ChangePct);

/// <summary>
/// A stored regression record for a run — the C# port of Rust
/// <c>RegressionRow</c>.
/// </summary>
public sealed record RegressionRow(
    Guid Id,
    Guid TestRunId,
    IReadOnlyList<Regression> Regressions,
    DateTime CreatedAt);

/// <summary>
/// A regression record joined to its config — the C# port of Rust
/// <c>RegressionWithConfig</c>.
/// </summary>
public sealed record RegressionWithConfig(
    Guid Id,
    Guid TestConfigId,
    string ConfigName,
    Guid TestRunId,
    IReadOnlyList<Regression> Regressions,
    DateTime CreatedAt);

/// <summary>
/// Benchmark regression detection — the C# counterpart of Rust
/// <c>crates/networker-dashboard/src/regression.rs</c>.
///
/// <para><b>Fidelity note (important):</b> the Rust <c>regression.rs</c> at the
/// pinned revision is a <b>v0.28.0 stub</b> — it defines only the three
/// serializable data shapes above and contains NO comparison logic, NO
/// thresholds, NO DB queries, and NO event emission (its doc comment says the
/// real comparison of <c>benchmark_artifact</c> rows vs a baseline "will" be
/// implemented). This port faithfully mirrors that: the data shapes are ported
/// 1:1, and <see cref="EmitRegressionEvent"/> is provided as the emission seam
/// so that <b>once the real comparison exists</b> (or is ported from the
/// pre-v0.28 module), the resulting regressions can be published on the event
/// bus as a <see cref="BenchmarkRegression"/> event — the same wire event the
/// Rust dashboard broadcasts. No thresholds are invented here (there are none to
/// port).</para>
///
/// <para><b>Where it SHOULD be called</b> (document only): from the run-complete
/// path (where a <c>test_run</c> finishes and its <c>benchmark_artifact</c> is
/// attached) — i.e. the dispatch/agent-result handler that marks a run complete.
/// After computing regressions vs the config's baseline, call
/// <see cref="EmitRegressionEvent"/> to notify the live dashboard.</para>
/// </summary>
public static class RegressionAnalyzer
{
    /// <summary>
    /// Publish a <see cref="BenchmarkRegression"/> event for a run's detected
    /// regressions. The <c>regressions</c> payload is serialized to JSON and
    /// forwarded verbatim (matching the Rust event's free-form
    /// <c>regressions</c> field). Returns the assigned event sequence number.
    /// </summary>
    public static long EmitRegressionEvent(
        EventBus bus,
        Guid configId,
        string configName,
        IReadOnlyList<Regression> regressions)
    {
        using var doc = JsonSerializer.SerializeToDocument(regressions);
        var evt = new BenchmarkRegression(
            configId,
            configName,
            regressions.Count,
            doc.RootElement.Clone());
        return bus.Publish(evt);
    }
}
