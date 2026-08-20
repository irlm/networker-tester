using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Networker.ControlPlane.Provisioning;
using Networker.ControlPlane.Realtime;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Issue #816: a failed matrix cell's only durable trace used to be
/// "install.sh exited with code 1" — the in-guest output #806 captures was
/// live-stream-only, so an unattended failure lost its diagnosis. These tests
/// pin the persistence contract that fixes that:
///
/// <list type="bullet">
///   <item><see cref="DeployRunner.DeployOutput"/> scrubs ANSI, tracks the
///     current "Step N: …" phase and the last fatal "✗ …" line, and BOUNDS the
///     accumulated log (the tail is the diagnosis; the row is returned by the
///     list endpoint).</item>
///   <item>A failed run persists exit_code + failed_step (V054) and an
///     error_message carrying the actionable detail — the orchestrator copies
///     that message onto the run row verbatim.</item>
/// </list>
///
/// <para>Joins the <c>cloud-cli-fake-bins</c> collection: every class that
/// points the process-wide <c>INSTALL_SH_PATH</c> at its own stub must run
/// serialized, or a parallel class's stub (and its exit code) answers this
/// class's spawn (#833 follow-up; seen as "expected exit 1, got 7").</para>
/// </summary>
[Collection("cloud-cli-fake-bins")]
public class DeployDiagnosticsTests
{
    private const string ProjectId = "p-diag";

    // ── DeployOutput unit behaviour ──────────────────────────────────────────

    [Fact]
    public void ProcessLine_strips_ansi_from_log_and_broadcast()
    {
        var output = new DeployRunner.DeployOutput();
        var broadcast = output.ProcessLine(
            "\u001b[0;31m  ✗\u001b[0m caddy install failed\u001b[0m", "stderr", out var clean);

        Assert.True(broadcast);
        Assert.Equal("  ✗ caddy install failed", clean);
        Assert.DoesNotContain('\u001b', output.FullLog);
        Assert.Contains("✗ caddy install failed", output.FullLog);
    }

    [Fact]
    public void ProcessLine_tracks_the_current_step_and_last_error_line()
    {
        var output = new DeployRunner.DeployOutput();
        output.ProcessLine("Step 6: Set up IIS", "stdout");
        output.ProcessLine("  ✗ transient thing that got retried", "stderr");
        output.ProcessLine("Step 7: Set up caddy on Windows VM", "stdout");
        output.ProcessLine("  → verifying ports…", "stdout");
        output.ProcessLine("  ✗ caddy is not serving on 8091/8454 after setup", "stderr");

        Assert.Equal("Step 7: Set up caddy on Windows VM", output.CurrentStep);
        Assert.Equal("caddy is not serving on 8091/8454 after setup", output.LastErrorLine);
    }

    [Fact]
    public void ProcessLine_captures_docker_path_error_lines()
    {
        var output = new DeployRunner.DeployOutput();
        output.ProcessLine("ERROR: docker run failed", "stderr");
        Assert.Equal("docker run failed", output.LastErrorLine);
    }

    [Fact]
    public void Log_is_bounded_keeping_the_tail()
    {
        var output = new DeployRunner.DeployOutput();
        // ~2x the cap in 100-char lines; the last line must survive.
        var filler = new string('x', 99);
        var lines = 2 * DeployRunner.DeployOutput.MaxLogChars / 100;
        for (var i = 0; i < lines; i++)
        {
            output.ProcessLine($"{filler}{i % 10}", "stdout");
        }
        output.ProcessLine("  ✗ the diagnosis lives in the tail", "stderr");

        var log = output.FullLog;
        Assert.True(log.Length <= DeployRunner.DeployOutput.MaxLogChars + 200,
            $"log not bounded: {log.Length} chars");
        Assert.StartsWith("[log truncated", log);
        Assert.EndsWith("✗ the diagnosis lives in the tail\n", log);
    }

    [Fact]
    public void BuildFailureMessage_carries_exit_code_detail_and_step()
    {
        var output = new DeployRunner.DeployOutput();
        output.ProcessLine("Step 7: Set up caddy on Windows VM", "stdout");
        output.ProcessLine("  ✗ caddy is not serving on 8091/8454 after setup", "stderr");

        var msg = DeployRunner.BuildFailureMessage(1, output);
        Assert.Equal(
            "install.sh exited with code 1 — caddy is not serving on 8091/8454 after setup " +
            "(during \"Step 7: Set up caddy on Windows VM\")",
            msg);
    }

    [Fact]
    public void BuildFailureMessage_degrades_to_the_bare_exit_code()
    {
        var msg = DeployRunner.BuildFailureMessage(null, new DeployRunner.DeployOutput());
        Assert.Equal("install.sh exited with code -1", msg);
    }

    // ── Terminal persistence (V054 columns) ──────────────────────────────────

    private static (ServiceProvider Sp, SqliteConnection Conn) BuildHost()
    {
        var conn = new SqliteConnection("DataSource=:memory:");
        conn.Open();

        var services = new ServiceCollection();
        services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
        services.AddSignalR();
        services.AddDashboardEventBus();
        services.AddSingleton(conn);
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(conn));

        var sp = services.BuildServiceProvider();
        RunDispatcherTesterFkTests.CreateMinimalSchema(conn);
        return (sp, conn);
    }

    private static DeployRunner Runner(IServiceProvider sp) => new(
        sp.GetRequiredService<IServiceScopeFactory>(),
        sp.GetRequiredService<EventBus>(),
        sp.GetRequiredService<ILogger<DeployRunner>>());

    private static Guid SeedDeployment(IServiceProvider sp)
    {
        using var scope = sp.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        var now = DateTime.UtcNow;
        db.Projects.Add(new Project
        {
            ProjectId = ProjectId,
            Name = "diag",
            Slug = "diag",
            Settings = "{}",
            CreatedAt = now,
            UpdatedAt = now,
        });
        var id = Guid.NewGuid();
        db.Deployments.Add(new Deployment
        {
            DeploymentId = id,
            Name = "target-azure-eastus-caddy-diag",
            Status = "pending",
            Config = "{}",
            CreatedAt = now,
            ProjectId = ProjectId,
        });
        db.SaveChanges();
        return id;
    }

    private static Deployment Row(IServiceProvider sp, Guid id)
    {
        using var scope = sp.CreateScope();
        var db = scope.ServiceProvider.GetRequiredService<NetworkerDbContext>();
        return db.Deployments.AsNoTracking().Single(d => d.DeploymentId == id);
    }

    /// <summary>Point INSTALL_SH_PATH at a throwaway script for the duration of
    /// <paramref name="body"/>, restoring the previous value afterwards.</summary>
    private static async Task WithInstallShAsync(string scriptBody, Func<Task> body)
    {
        var path = Path.Combine(Path.GetTempPath(), $"install-stub-{Guid.NewGuid():N}.sh");
        await File.WriteAllTextAsync(path, scriptBody);
        var previous = Environment.GetEnvironmentVariable("INSTALL_SH_PATH");
        Environment.SetEnvironmentVariable("INSTALL_SH_PATH", path);
        try
        {
            await body();
        }
        finally
        {
            Environment.SetEnvironmentVariable("INSTALL_SH_PATH", previous);
            try { File.Delete(path); } catch { /* best-effort */ }
        }
    }

    [Fact]
    public async Task Failed_deploy_persists_exit_code_failed_step_and_actionable_error()
    {
        var (sp, conn) = BuildHost();
        await using var _ = sp;
        using var __ = conn;

        var id = SeedDeployment(sp);

        await WithInstallShAsync(
            "#!/usr/bin/env bash\n" +
            "echo 'Step 7: Set up caddy on Windows VM'\n" +
            "echo '  ✗ caddy is not serving on 8091/8454 after setup' >&2\n" +
            "exit 1\n",
            async () =>
            {
                await Runner(sp).RunDeploymentAsync(id, "{}", CancellationToken.None);
            });

        var row = Row(sp, id);
        Assert.Equal("failed", row.Status);
        Assert.Equal(1, row.ExitCode);
        Assert.Equal("Step 7: Set up caddy on Windows VM", row.FailedStep);
        // The run row inherits this message verbatim ("Provisioning failed: …")
        // — it must carry the actionable line, not just the exit code.
        Assert.Contains("exited with code 1", row.ErrorMessage);
        Assert.Contains("caddy is not serving on 8091/8454 after setup", row.ErrorMessage);
        Assert.Contains("Step 7: Set up caddy on Windows VM", row.ErrorMessage);
        // The log survives for post-mortem (and the SSE events replay).
        Assert.Contains("caddy is not serving", row.Log);
    }

    [Fact]
    public async Task Successful_deploy_persists_exit_code_zero_and_no_failed_step()
    {
        var (sp, conn) = BuildHost();
        await using var _ = sp;
        using var __ = conn;

        var id = SeedDeployment(sp);

        await WithInstallShAsync(
            "#!/usr/bin/env bash\n" +
            "echo 'Step 1: Deploy endpoints'\n" +
            "echo 'endpoint_ip: 20.1.2.3'\n" +
            "exit 0\n",
            async () =>
            {
                await Runner(sp).RunDeploymentAsync(id, "{}", CancellationToken.None);
            });

        var row = Row(sp, id);
        Assert.Equal("completed", row.Status);
        Assert.Equal(0, row.ExitCode);
        Assert.Null(row.FailedStep);
        Assert.Null(row.ErrorMessage);
    }
}
