using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Primitives;
using Networker.ControlPlane.Endpoints;
using Networker.ControlPlane.Observability;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The <c>service_log</c> sink's pure parts: the level encoding it shares with
/// the Rust writers and the C# reader, option parsing, and the metric counters
/// <c>/api/logs/pipeline-status</c> publishes. DB-free.
/// </summary>
public sealed class ServiceLogSinkTests
{
    // ── level encoding ──────────────────────────────────────────────────────
    // This is the contract between three parties: networker_log::Level::as_db
    // (the Rust tester/endpoint writers), this sink, and LogsEndpoints'
    // level filter. Getting it wrong silently files every row under the wrong
    // severity, which no test of this sink alone would notice.

    [Theory]
    [InlineData(LogLevel.Critical, 1)]
    [InlineData(LogLevel.Error, 1)]
    [InlineData(LogLevel.Warning, 2)]
    [InlineData(LogLevel.Information, 3)]
    [InlineData(LogLevel.Debug, 4)]
    [InlineData(LogLevel.Trace, 5)]
    public void Db_level_matches_the_rust_encoding(LogLevel level, short expected)
    {
        Assert.Equal(expected, ServiceLogOptions.ToDbLevel(level));
    }

    [Fact]
    public void Writer_and_reader_agree_on_every_level()
    {
        // What this sink WRITES for a level must be what the read side's filter
        // SELECTS for the same name — otherwise "level=error" in the UI returns
        // rows this process filed under something else.
        foreach (var (level, name) in new[]
                 {
                     (LogLevel.Error, "error"),
                     (LogLevel.Warning, "warn"),
                     (LogLevel.Information, "info"),
                     (LogLevel.Debug, "debug"),
                     (LogLevel.Trace, "trace"),
                 })
        {
            Assert.Equal(
                LogsEndpoints.ParseLevelToDb(name),
                ServiceLogOptions.ToDbLevel(level));
        }
    }

    [Fact]
    public void Critical_folds_into_error_because_the_table_has_no_sixth_severity()
    {
        // SMALLINT 1..5 is the whole vocabulary; a 0 or 6 would be invisible to
        // every level filter in the UI.
        Assert.Equal(ServiceLogOptions.ToDbLevel(LogLevel.Error), ServiceLogOptions.ToDbLevel(LogLevel.Critical));
        foreach (var level in new[]
                 {
                     LogLevel.Critical, LogLevel.Error, LogLevel.Warning,
                     LogLevel.Information, LogLevel.Debug, LogLevel.Trace,
                 })
        {
            var db = ServiceLogOptions.ToDbLevel(level);
            Assert.InRange(db, (short)1, (short)5);
        }
    }

    // ── options ─────────────────────────────────────────────────────────────

    [Fact]
    public void Sink_is_off_unless_explicitly_enabled()
    {
        // The default must be OFF: enabling it turns every qualifying log line
        // into a database write, which is not a decision to make for someone.
        var options = Options([]);
        Assert.False(options.Enabled);
    }

    [Theory]
    [InlineData("1", true)]
    [InlineData("true", true)]
    [InlineData("TRUE", true)]
    [InlineData("yes", true)]
    [InlineData("on", true)]
    [InlineData("0", false)]
    [InlineData("false", false)]
    [InlineData("", false)]
    [InlineData("maybe", false)]
    public void Enabled_accepts_the_usual_truthy_spellings(string raw, bool expected)
    {
        Assert.Equal(expected, ServiceLogOptions.ParseBool(raw));
    }

    [Fact]
    public void Defaults_are_the_documented_ones()
    {
        var o = Options(new() { [ServiceLogOptions.EnabledEnvVar] = "1" });

        Assert.True(o.Enabled);
        Assert.Equal(LogLevel.Information, o.MinimumLevel);   // Debug/Trace is a firehose
        Assert.Equal("dashboard", o.Service);                 // what the UI's filter lists
        Assert.Equal(10_000, o.QueueCapacity);
        Assert.Equal(200, o.BatchSize);
        Assert.Equal(7, o.RetentionDays);
    }

    [Fact]
    public void Overrides_are_read_and_clamped()
    {
        var o = Options(new()
        {
            [ServiceLogOptions.EnabledEnvVar] = "1",
            [ServiceLogOptions.LevelEnvVar] = "warn",
            [ServiceLogOptions.ServiceEnvVar] = " orchestrator ",
            [ServiceLogOptions.BatchEnvVar] = "999999",   // above the cap
            [ServiceLogOptions.QueueEnvVar] = "1",        // below the floor
        });

        Assert.Equal(LogLevel.Warning, o.MinimumLevel);
        Assert.Equal("orchestrator", o.Service);
        Assert.Equal(10_000, o.BatchSize);
        Assert.Equal(100, o.QueueCapacity);
    }

    [Fact]
    public void A_typo_keeps_the_default_rather_than_stopping_the_process()
    {
        // A malformed env var must never be fatal: the control plane serving
        // traffic matters more than the log level being exactly right.
        var o = Options(new()
        {
            [ServiceLogOptions.EnabledEnvVar] = "1",
            [ServiceLogOptions.LevelEnvVar] = "verbose",       // not a level
            [ServiceLogOptions.IntervalEnvVar] = "not-a-number",
        });

        Assert.Equal(LogLevel.Information, o.MinimumLevel);
        Assert.Equal(TimeSpan.FromSeconds(2), o.FlushInterval);
    }

    // ── metrics ─────────────────────────────────────────────────────────────

    [Fact]
    public void Metrics_track_queue_depth_across_enqueue_and_flush()
    {
        var m = new ServiceLogMetrics();
        for (var i = 0; i < 5; i++) m.OnEnqueued();
        Assert.Equal(5, m.QueueDepth);

        m.OnFlushed(rows: 3, elapsedMs: 12);
        Assert.Equal(3, m.EntriesWritten);
        Assert.Equal(2, m.QueueDepth);
        Assert.Equal(1, m.FlushCount);
        Assert.Equal(12, m.LastFlushMs);
        Assert.Equal(0, m.EntriesDropped);
    }

    [Fact]
    public void A_failed_flush_counts_its_rows_as_dropped_not_written()
    {
        // Rows leave the queue on failure — retrying forever would turn a
        // database outage into an unbounded memory leak — so they must be
        // reported honestly as lost, never as written.
        var m = new ServiceLogMetrics();
        m.OnEnqueued(); m.OnEnqueued();
        m.OnFlushFailed(rows: 2);

        Assert.Equal(0, m.EntriesWritten);
        Assert.Equal(2, m.EntriesDropped);
        Assert.Equal(1, m.FlushErrors);
        Assert.Equal(0, m.QueueDepth);
    }

    [Fact]
    public void Overflow_is_counted_as_dropped()
    {
        var m = new ServiceLogMetrics();
        m.OnDropped();
        Assert.Equal(1, m.EntriesDropped);
        Assert.Equal(0, m.QueueDepth);   // never queued, so depth is untouched
    }

    /// <summary>A minimal IConfiguration over a dictionary — enough for
    /// FromEnvironment, without pulling a configuration package into this test
    /// project just to build one.</summary>
    private sealed class DictConfig(Dictionary<string, string?> values) : IConfiguration
    {
        public string? this[string key]
        {
            get => values.GetValueOrDefault(key);
            set => values[key] = value;
        }

        public IEnumerable<IConfigurationSection> GetChildren() => [];
        public IChangeToken GetReloadToken() => throw new NotSupportedException();
        public IConfigurationSection GetSection(string key) => throw new NotSupportedException();
    }

    private static ServiceLogOptions Options(Dictionary<string, string?> values) =>
        ServiceLogOptions.FromEnvironment(new DictConfig(values));
}
