using Microsoft.AspNetCore.SignalR;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Contracts;
using Networker.Data;

// Phase 2 proof-of-concept control plane.
//
// This is the re-architecture, not a transliteration: the Rust dashboard's
// hand-written SQL + manual row mapping is replaced by an EF Core model
// (reverse-engineered database-first from the SAME live schema), and the
// three hand-rolled WebSocket hubs are replaced by SignalR. The endpoints
// below mirror a slice of the existing REST contract so the React frontend
// and agents wouldn't notice the swap.
//
// Deliberately minimal — it proves EF reads the real schema and SignalR
// stands up. It is NOT the full control plane.

var builder = WebApplication.CreateBuilder(args);

var connString = builder.Configuration.GetConnectionString("Networker")
    ?? Environment.GetEnvironmentVariable("DASHBOARD_DB_URL_NPGSQL")
    ?? "Host=127.0.0.1;Port=5432;Database=networker_core;Username=networker;Password=networker";

// EF Core replaces tokio-postgres + ~40 hand-written migrations. The model is
// the source of truth; "queried a dropped table" becomes a compile error.
builder.Services.AddDbContext<NetworkerDbContext>(o => o.UseNpgsql(connString));

// SignalR replaces the 3 hand-rolled axum WS hubs (agent/browser/tester).
// Reconnection, groups, and a Redis backplane are framework features here.
builder.Services.AddSignalR();

// Phase-2 M0: JWT auth + RBAC foundation. Interchangeable with the Rust
// dashboard's HS256/DASHBOARD_JWT_SECRET scheme. Reads dash_user/project_member
// via raw SQL (Npgsql), independent of the EF model. Additive only — existing
// routes below stay unauthenticated; endpoints opt in via RequireAuthorization.
builder.Services.AddNetworkerAuth(connString);

var app = builder.Build();

// Order matters: authentication → DB-status middleware → authorization, all
// after routing so {projectId} route values reach the project-scope handler.
app.UseNetworkerAuth();

// GET /api/health — same shape the Rust dashboard serves; used by the deploy
// health check and the frontend connection dot.
app.MapGet("/api/health", async (NetworkerDbContext db) =>
{
    var dbOk = await db.Database.CanConnectAsync();
    return Results.Ok(new
    {
        status = dbOk ? "ok" : "degraded",
        version = "hybrid-phase2-poc",
        db = dbOk ? "ok" : "error",
    });
});

// GET /api/projects/{projectId}/testers — LINQ replaces hand-written SELECT +
// manual row.get() mapping. Compile-checked against the model.
app.MapGet("/api/projects/{projectId}/testers", async (string projectId, NetworkerDbContext db) =>
{
    var testers = await db.ProjectTesters
        .AsNoTracking()
        .Where(t => t.ProjectId == projectId)
        .OrderByDescending(t => t.CreatedAt)
        .Select(t => new
        {
            tester_id = t.TesterId,
            name = t.Name,
            cloud = t.Cloud,
            region = t.Region,
            vm_size = t.VmSize,
            power_state = t.PowerState,
            allocation = t.Allocation,
        })
        .ToListAsync();
    return Results.Ok(testers);
});

// GET /api/projects/{projectId}/test-runs — joins TestRun→TestConfig for the
// name, the same shape the Runs page consumes.
app.MapGet("/api/projects/{projectId}/test-runs", async (string projectId, int? limit, NetworkerDbContext db) =>
{
    var runs = await db.TestRuns
        .AsNoTracking()
        .Where(r => r.ProjectId == projectId)
        .OrderByDescending(r => r.CreatedAt)
        .Take(limit ?? 20)
        .Join(db.TestConfigs, r => r.TestConfigId, c => c.Id, (r, c) => new
        {
            id = r.Id,
            name = c.Name,
            status = r.Status,
            success_count = r.SuccessCount,
            failure_count = r.FailureCount,
            tester_id = r.TesterId,
            created_at = r.CreatedAt,
        })
        .ToListAsync();
    return Results.Ok(runs);
});

// SignalR hub for live dashboard updates. Phase 2 wires run/tester events
// through Groups($"project:{id}") — for now the hub just stands up so the
// endpoint negotiates. The agent↔dashboard hub is the same pattern.
app.MapHub<DashboardHub>("/ws/dashboard");

// Agent-facing hub. Replaces the Rust `ws/agent_hub.rs` — SignalR handles the
// connection lifecycle, reconnection, and (with a backplane) multi-replica
// routing the Rust code maintained by hand.
app.MapHub<AgentHub>("/ws/agent");

// POST /auth/login + GET /auth/profile — same response shapes the Rust
// dashboard serves. The policies (GlobalAdmin/Operator/Viewer, ProjectMember/
// Operator/Admin) are registered and available for other endpoints to opt into.
app.MapAuthEndpoints();

app.Run();

// Exposes the top-level-statement Program to WebApplicationFactory<Program>
// for integration tests (the standard minimal-API testing pattern).
public partial class Program { }

/// Browser-facing live-updates hub. In full Phase 2 the control plane calls
/// Clients.Group($"project:{id}").RunUpdated(...) instead of the Rust code's
/// hand-maintained connection map.
public class DashboardHub : Hub
{
    public Task Subscribe(string projectId) =>
        Groups.AddToGroupAsync(Context.ConnectionId, $"project:{projectId}");
}

/// Agent → control plane hub. Agents invoke <c>ReportResult</c>/<c>Heartbeat</c>;
/// the hub logs them and fans the result out to any browser watching the same
/// project via the <see cref="DashboardHub"/> group — the Rust code hand-wired
/// this cross-hub routing with a shared connection map. In full Phase 2,
/// ReportResult also persists via EF Core.
public class AgentHub(ILogger<AgentHub> logger, IHubContext<DashboardHub> dashboard) : Hub
{
    public async Task ReportResult(ProbeRunResult result)
    {
        logger.LogInformation(
            "Agent {ConnId} reported run {RunId}: schema_version={Schema}, {Count} attempts, target={Target}",
            Context.ConnectionId, result.RunId, result.SchemaVersion, result.Attempts.Count, result.TargetUrl);

        // Fan out to browsers (best-effort demo of the cross-hub path).
        await dashboard.Clients.All.SendAsync("RunReported", new
        {
            result.RunId,
            result.TargetUrl,
            result.SchemaVersion,
            attempts = result.Attempts.Count,
        });
    }

    public Task Heartbeat(string agentName)
    {
        logger.LogInformation("Heartbeat from agent {AgentName} ({ConnId})", agentName, Context.ConnectionId);
        return Task.CompletedTask;
    }

    public override Task OnConnectedAsync()
    {
        logger.LogInformation("Agent connected: {ConnId}", Context.ConnectionId);
        return base.OnConnectedAsync();
    }
}
