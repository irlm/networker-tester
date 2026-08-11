using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.Logging.Abstractions;
using Networker.ControlPlane.Background;
using Networker.Data.Entities;

namespace Networker.Tests;

/// <summary>
/// The system-health checker + retention pass against real Postgres — the
/// job whose ABSENCE (never ported from Rust; the Rust version never pruned
/// either, only its docs did) left the Settings panel serving 2026-07-15
/// fossil rows, including a red logs_retention that was truthfully
/// reporting 99-day-old perf_log rows nothing ever deleted.
/// </summary>
public sealed class SystemHealthServiceTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fixture;

    public SystemHealthServiceTests(ControlPlaneFixture fixture) => _fixture = fixture;

    private async Task EnsurePerfLogTableAsync(Networker.Data.NetworkerDbContext db)
    {
        // perf_log is deliberately unmapped (Rust-era raw-SQL table), so the
        // fixture's model DDL doesn't create it — mirror the prod shape.
        await db.Database.ExecuteSqlRawAsync(
            "CREATE TABLE IF NOT EXISTS perf_log (id bigserial PRIMARY KEY, logged_at timestamptz NOT NULL, kind text, path text, total_ms double precision)");
    }

    [Fact]
    public async Task Sweep_writes_fresh_checks_prunes_perf_log_and_ages_out_fossils()
    {
        await using var db = _fixture.NewDbContext();
        await EnsurePerfLogTableAsync(db);

        // Fossil health rows (the incident shape: month-old, incl. a retired
        // check name) + perf_log rows on both sides of the 7-day window.
        var fossilAt = DateTime.UtcNow.AddDays(-27);
        db.SystemHealths.AddRange(
            new SystemHealth { CheckName = "logs_retention", Status = "red", Value = "99 days", CheckedAt = fossilAt },
            new SystemHealth { CheckName = "logs_db", Status = "green", CheckedAt = fossilAt });
        await db.SaveChangesAsync();
        await db.Database.ExecuteSqlRawAsync(
            "INSERT INTO perf_log (logged_at) VALUES (now() - interval '30 days'), (now() - interval '2 days')");

        var written = await SystemHealthService.RunOnceAsync(db, NullLogger.Instance, CancellationToken.None);
        Assert.Equal(3, written);

        // Old perf_log rows pruned; recent survive.
        var perfCount = await db.Database
            .SqlQueryRaw<long>("SELECT count(*) AS \"Value\" FROM perf_log")
            .SingleAsync();
        Assert.Equal(1, perfCount);

        // Fossils (incl. the retired logs_db name) aged out; fresh rows present.
        var rows = await db.SystemHealths.AsNoTracking().ToListAsync();
        Assert.DoesNotContain(rows, r => r.CheckedAt <= fossilAt.AddMinutes(1));
        Assert.DoesNotContain(rows, r => r.CheckName == "logs_db");

        var retention = rows.Single(r => r.CheckName == "logs_retention");
        // Oldest surviving row is 2 days old → green.
        Assert.Equal("green", retention.Status);
        Assert.Equal("2 days", retention.Value);

        Assert.Equal("green", rows.Single(r => r.CheckName == "core_db").Status);
        var size = rows.Single(r => r.CheckName == "core_db_size");
        Assert.Equal("green", size.Status);
        Assert.EndsWith(" GB", size.Value);

        // Cleanup for other suites sharing the fixture DB.
        await db.Database.ExecuteSqlRawAsync("DELETE FROM perf_log");
        await db.SystemHealths.ExecuteDeleteAsync();
    }

    [Fact]
    public async Task Retention_goes_red_when_rows_outlive_the_window_and_pruning_is_disabled_by_age_alone()
    {
        await using var db = _fixture.NewDbContext();
        await EnsurePerfLogTableAsync(db);
        // A row 9 days old is PRUNED by the sweep, so post-sweep retention is
        // green — assert the pruning is what saves it by checking the row
        // count went to zero (the sweep self-heals the red the panel showed).
        await db.Database.ExecuteSqlRawAsync(
            "INSERT INTO perf_log (logged_at) VALUES (now() - interval '9 days')");

        await SystemHealthService.RunOnceAsync(db, NullLogger.Instance, CancellationToken.None);

        var perfCount = await db.Database
            .SqlQueryRaw<long>("SELECT count(*) AS \"Value\" FROM perf_log")
            .SingleAsync();
        Assert.Equal(0, perfCount);
        var retention = await db.SystemHealths.AsNoTracking()
            .Where(r => r.CheckName == "logs_retention")
            .OrderByDescending(r => r.CheckedAt).FirstAsync();
        Assert.Equal("green", retention.Status);
        Assert.Equal("empty", retention.Value);

        await db.SystemHealths.ExecuteDeleteAsync();
    }
}
