using Microsoft.AspNetCore.SignalR;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Background;
using Networker.ControlPlane.Dispatch;
using Networker.ControlPlane.Endpoints;
using Networker.ControlPlane.Realtime;
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

// Phase-2 M2 realtime: the browser event bus (/ws/dashboard, with replay+seq)
// and the tester-queue hub (/ws/testers, project-scoped subscriptions). JWT for
// these WebSocket hubs arrives as ?access_token= (browsers can't set the header)
// — AddNetworkerAuth wires JwtBearer to read it for /ws paths.
builder.Services.AddDashboardEventBus();
builder.Services.AddTesterQueueHub();
// M2 slice 2: the agent hub connection registry + outbound sender. Agents
// authenticate by api_key (not JWT) inside the hub's OnConnectedAsync.
builder.Services.AddAgentProtocol();
// M3 write path: the run dispatcher — creates test_run rows on launch and
// assigns them to an online agent via the AgentConnectionRegistry, with
// queued-run redispatch. Drives the core benchmarking loop.
builder.Services.AddRunDispatcher();
// M3 slice 2: background services (hosted). Scheduler fires due schedules →
// LaunchAsync; the redispatcher retries stuck-queued runs; the watchdog fails
// runs whose agent died; the reaper marks dead agents offline (the live agent
// registry is the authoritative liveness signal).
builder.Services.AddNetworkerSchedulerServices();
builder.Services.AddNetworkerReconciliationServices();

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

// M1 read-only endpoint parity — the hot GET endpoints the React frontend
// consumes, ported from the Rust dashboard as EF/LINQ against the full model
// and gated by the M0 project-scope policies. Each module is a static
// extension (src/Networker.ControlPlane/Endpoints/) so the surface can grow
// per-domain without this file churning. These supersede the two PoC inline
// endpoints that used to live here (testers + test-runs).
app.MapProjectsEndpoints();
app.MapTestersEndpoints();
app.MapTestRunsEndpoints();
app.MapTestConfigsEndpoints();
app.MapAgentsEndpoints();
app.MapDeploymentsEndpoints();
app.MapPlatformEndpoints();

// M3 write path — create/patch/delete + launch configs, cancel runs, and
// schedules + comparison-groups CRUD (trigger/launch shells wired to the
// dispatcher in M3 slice 2 alongside the scheduler background service).
app.MapTestConfigWriteEndpoints();
app.MapTestRunWriteEndpoints();
app.MapSchedulesEndpoints();
app.MapComparisonGroupsEndpoints();

// M2 browser event bus — live dashboard updates with replay + sequence numbers
// (the Rust EventBus + browser_hub, ported). Clients connect with
// ?access_token=<jwt>[&since=<seq>] and catch up via replay before tailing live.
app.MapHub<BrowserHub>("/ws/dashboard");

// M2 tester-queue hub — project-scoped per-tester running/queued updates,
// subscribe/unsubscribe with membership checks + rate limits.
app.MapHub<TesterQueueHub>("/ws/testers");

// M2 agent hub — the full agent↔control-plane protocol (ws/agent_hub.rs ported):
// api-key auth in OnConnectedAsync, EF persistence per inbound message, run
// events published through the EventBus, orphan-run failing on disconnect, and a
// connection registry the M3 dispatcher pushes AssignRun/CancelRun through.
app.MapHub<AgentProtocolHub>("/ws/agent");

// POST /auth/login + GET /auth/profile — same response shapes the Rust
// dashboard serves. The policies (GlobalAdmin/Operator/Viewer, ProjectMember/
// Operator/Admin) are registered and available for other endpoints to opt into.
app.MapAuthEndpoints();

app.Run();

// Exposes the top-level-statement Program to WebApplicationFactory<Program>
// for integration tests (the standard minimal-API testing pattern).
public partial class Program { }
