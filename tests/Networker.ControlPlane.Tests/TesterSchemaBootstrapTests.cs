using Networker.ControlPlane.Realtime.RawWs;
using Xunit;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Guards the lazy tester-probe-schema bootstrap the attempt ingest performs
/// (<see cref="AttemptPersister"/>): the embedded copy of
/// <c>shared/tester-schema.postgres.sql</c> must be present, must be the SAME
/// bytes as the repo file (the Rust side guards that file against
/// <c>postgres.rs</c>), and must contain every table the persister writes.
/// A fresh control-plane database used to persist zero attempts forever
/// because nothing created <c>RequestAttempt</c> on the streamed path
/// (found by <c>lab/validate.sh</c>).
/// </summary>
public class TesterSchemaBootstrapTests
{
    private static readonly string[] RequiredTables =
    [
        "TestRun", "RequestAttempt", "DnsResult", "TcpResult", "TlsResult",
        "HttpResult", "UdpResult", "ServerTimingResult", "ErrorRecord",
        "MthroughputResult",
    ];

    [Fact]
    public void EmbeddedSchemaMatchesSharedFile()
    {
        var onDisk = File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "shared", "tester-schema.postgres.sql"));
        Assert.Equal(onDisk, AttemptPersister.EmbeddedTesterSchema);
    }

    [Fact]
    public void EmbeddedSchemaDeclaresEveryTableThePersisterWrites()
    {
        var sql = AttemptPersister.EmbeddedTesterSchema;
        foreach (var table in RequiredTables)
        {
            Assert.Contains($"CREATE TABLE IF NOT EXISTS {table} (", sql);
        }
        // V005 column the persister writes when the schema is available.
        Assert.Contains("ALTER TABLE ServerTimingResult ADD COLUMN IF NOT EXISTS SrvCpuMs", sql);
    }

    [Fact]
    public void EmbeddedSchemaIsIdempotentDdlOnly()
    {
        // Every CREATE must be guarded — the ingest re-runs this on every process
        // start against databases the tester (or install.sh) already created.
        foreach (var line in AttemptPersister.EmbeddedTesterSchema.Split('\n'))
        {
            var t = line.Trim();
            if (t.StartsWith("CREATE TABLE", StringComparison.Ordinal))
            {
                Assert.StartsWith("CREATE TABLE IF NOT EXISTS", t);
            }
            if (t.StartsWith("CREATE INDEX", StringComparison.Ordinal) || t.StartsWith("CREATE UNIQUE INDEX", StringComparison.Ordinal))
            {
                Assert.Contains("IF NOT EXISTS", t);
            }
            if (t.StartsWith("ALTER TABLE", StringComparison.Ordinal) && t.Contains("ADD COLUMN", StringComparison.Ordinal))
            {
                Assert.Contains("ADD COLUMN IF NOT EXISTS", t);
            }
        }
    }

    // ── Fielded-schema shape detection ───────────────────────────────────────

    [Fact]
    public void Shape_LiveProdSchema_UsesExtrajsonAndMinimalTestRun()
    {
        // The prod schema the persister was dry-run-verified against (2026-07-28):
        // RequestAttempt.extrajson exists; testrun's V001 NOT NULL columns do not
        // bite (kept minimal — only what exists is filled).
        var shape = AttemptPersister.DeriveShape(
        [
            ("requestattempt", "attemptid"), ("requestattempt", "extrajson"),
            ("testrun", "runid"), ("testrun", "targeturl"), ("testrun", "targethost"),
        ]);
        Assert.Equal("extrajson", shape.ExtraJsonColumn);
        Assert.Empty(shape.TestRunColumns);
    }

    [Fact]
    public void Shape_InstallShSeed_UsesExtraJsonUnderscoreAndFullTestRun()
    {
        var shape = AttemptPersister.DeriveShape(
        [
            ("RequestAttempt", "AttemptId"), ("RequestAttempt", "extra_json"),
            ("TestRun", "RunId"), ("TestRun", "StartedAt"), ("TestRun", "Modes"),
            ("TestRun", "ClientOs"), ("TestRun", "ClientVersion"),
        ]);
        Assert.Equal("extra_json", shape.ExtraJsonColumn);
        Assert.Equal(new[] { "clientos", "clientversion", "modes", "startedat" }, shape.TestRunColumns.Order());
    }

    [Fact]
    public void Shape_FreshBootstrap_HasNoExtraColumnAndFullTestRun()
    {
        // What the embedded V001–V005 creates: no raw-attempt JSON column at all.
        var shape = AttemptPersister.DeriveShape(
        [
            ("requestattempt", "attemptid"), ("requestattempt", "retrycount"),
            ("testrun", "runid"), ("testrun", "startedat"), ("testrun", "modes"),
            ("testrun", "clientos"), ("testrun", "clientversion"),
        ]);
        Assert.Null(shape.ExtraJsonColumn);
        Assert.Equal(4, shape.TestRunColumns.Count);
    }

    [Fact]
    public void Shape_PrefersExtrajsonWhenBothColumnsExist()
    {
        var shape = AttemptPersister.DeriveShape(
            [("requestattempt", "extra_json"), ("requestattempt", "extrajson")]);
        Assert.Equal("extrajson", shape.ExtraJsonColumn);
    }
}
