using Microsoft.AspNetCore.SignalR;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane;
using Networker.ControlPlane.Alerting;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Background;
using Networker.ControlPlane.Dispatch;
using Networker.ControlPlane.Endpoints;
using Networker.ControlPlane.Realtime;
using Networker.ControlPlane.Realtime.RawWs;
using Networker.ControlPlane.Notifications;
using Networker.ControlPlane.Provisioning;
using Networker.ControlPlane.Security;
using Networker.ControlPlane.Sso;
using Networker.Contracts;
using Networker.Data;
using Networker.Data.Migrations;

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
// M4 provisioning: the credential cipher (encrypts cloud-account secrets,
// byte-compatible with the Rust dashboard) and the compute provisioner (CLI
// shell-out to az/aws/gcloud for VM lifecycle — SDKs are a later pass).
builder.Services.AddCredentialCipher();
builder.Services.AddComputeProvisioner();
// M4 slice 2: the provisioning orchestrator (Pending→provision→Network→re-queue,
// via the deploy-runner shelling install.sh) and the cloud lifecycle loops
// (auto-shutdown of idle testers, orphan-resource reaper).
builder.Services.AddProvisioningOrchestrator();
// Benchmark-catalog language detection: SSH probe of /opt/bench/* installs
// (port of the Rust ssh_detect_languages — key-auth ssh shell-out).
builder.Services.AddSingleton<ISshLanguageDetector, SshLanguageDetector>();
builder.Services.AddNetworkerCloudLifecycleServices();
// M5 admin/orgs/SSO: the workspace-inactivity lifecycle loop and the SSO
// module (OIDC flows + provider admin, provider secrets encrypted via the cipher).
builder.Services.AddNetworkerInactivityService();
builder.Services.AddSsoModule();
// Phase-3 completeness: email sender (ACS or no-op), VM-lifecycle audit recorder,
// GitHub version-refresh cache. AddTesterLoops (legacy benchmark_config dispatch/
// recovery loops) is intentionally NOT wired — the unified C# schema has no
// benchmark_config table; the M3 dispatcher/redispatcher own run assignment.
builder.Services.AddNetworkerEmailSender();
builder.Services.AddVmLifecycleRecorder();
// Alerting (wave 1): threshold rules + notification channels. The evaluator
// hooks run_finished in AgentMessageProcessor (both transports) and delivers
// via webhook (HMAC-signed) or the email sender registered above.
builder.Services.AddNetworkerAlerting();
// Floor = the real assembly version (Directory.Build.props, single-sourced
// with Cargo.toml) — never a hardcoded string.
builder.Services.AddVersionRefresh(VersionEndpoints.DashboardVersion);
// M6 cutover: raw-WebSocket bridges (the React frontend + fielded Rust agents
// speak raw WS JSON, not SignalR) + per-tick pg-advisory leader election and
// tick observability for the background loops. AddRawWebSockets must come
// after AddSignalR (it decorates the tester-queue hub lifetime manager).
builder.Services.AddRawWebSockets();
builder.Services.AddAgentRawSocket();
builder.Services.AddOpsInfrastructure();

var app = builder.Build();

// Schema migrations at startup (docs/schema-ownership.md follow-up): with the
// Rust dashboard retired, this process boots first, so it owns applying the
// V0NN chain. On an already-migrated database this is a no-op (bookkeeping
// rows short-circuit); on failure the app refuses to start — the deploy's
// readiness check then rolls back the build. NETWORKER_RUN_MIGRATIONS=0 opts
// out (used by test hosts that materialize the schema themselves).
if (builder.Configuration["NETWORKER_RUN_MIGRATIONS"] != "0")
{
    var migrationResult = await SchemaMigrator.MigrateAsync(connString);
    app.Logger.LogInformation(
        "Schema migrations: {Applied} applied, {Existing} already recorded (latest V{Latest:D3})",
        migrationResult.Applied.Count, migrationResult.AlreadyApplied.Count, SchemaMigrator.LatestVersion);
}

// Global 500 contract — FIRST middleware so any unhandled exception below
// (auth, endpoints, raw WS) becomes the uniform { "error": ... } envelope
// with a server-side log, instead of Kestrel's undefined empty 500. Handled
// 4xx envelopes are untouched (this only fires on unhandled exceptions).
app.UseErrorEnvelope();

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
        version = VersionEndpoints.DashboardVersion,
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

// M4 provisioning + VM lifecycle — cloud credential management (encrypted) and
// tester start/stop/upgrade/probe/postpone/schedule/force-stop/delete (202-async,
// cloud calls behind IComputeProvisioner). Pending→provision (deploy-runner +
// orchestrator) is M4 slice 2.
app.MapCloudAccountsEndpoints();
app.MapCloudConnectionsEndpoints();
app.MapTesterWriteEndpoints();
app.MapDeploymentWriteEndpoints();

// M5 admin / orgs / access-control / SSO — users + admin + project write;
// members/invites/share-links/visibility; approvals + agent-commands + catalog;
// account/password + SSO flows + provider admin.
app.MapUsersEndpoints();
app.MapAdminEndpoints();
app.MapProjectWriteEndpoints();
app.MapMembersEndpoints();
app.MapInvitesEndpoints();
app.MapShareLinksEndpoints();
app.MapVisibilityRulesEndpoints();
app.MapApprovalsEndpoints();
app.MapAgentCommandsEndpoints();
app.MapBenchmarkCatalogEndpoints();
app.MapAccountEndpoints();
app.MapSsoEndpoints();
app.MapSsoAdminEndpoints();

// M6 raw-WebSocket surface — the CUTOVER transport. The React frontend
// (new WebSocket + JSON.parse) and the fielded Rust agents (tungstenite text
// frames) connect UNMODIFIED to the same /ws/* paths the Rust dashboard served:
//   /ws/dashboard?token=[&since=]  — event feed with replay + seq
//   /ws/testers?token=             — subscribe_tester_queue / snapshots / updates
//   /ws/agent?key=                 — the full agent protocol
app.UseWebSockets();
app.MapRawWebSockets();
app.MapAgentRawSocket();

// The SignalR hubs stay available at /hub/* for future SignalR-native clients
// (e.g. the C# agent skeleton). Same underlying processors/registries — the
// two transports share seq streams, connection registry, and persistence.
app.MapHub<BrowserHub>("/hub/dashboard");
app.MapHub<TesterQueueHub>("/hub/testers");
app.MapHub<AgentProtocolHub>("/hub/agent");

// M6 ops surface: background-service tick health + readiness probe.
app.MapOpsEndpoints();

// Phase-3: the remaining REST modules, completing control-plane parity with the
// Rust dashboard so its crate can be retired. Auth is applied per-route inside
// each module (public leaderboard; admin update/system-health/perf-log; project-
// scoped url-tests/tls-profiles/inventory/precheck; flat logs/bench-tokens).
app.MapLeaderboardEndpoints();
app.MapSystemHealthEndpoints();
app.MapLogsEndpoints();
app.MapPerfLogEndpoints();
app.MapUpdateEndpoints();
app.MapBenchTokensEndpoints();
app.MapUrlTestsEndpoints();
app.MapTlsProfilesEndpoints();
// Alerting (wave 1): channels + rules CRUD, event history, channel test-fire.
app.MapAlertsEndpoints();
app.MapInventoryEndpoints();
app.MapTesterPrecheckEndpoints();
// GET /api/version — the frontend's "Latest version" toast + tester-upgrade
// badge. Reads the LatestVersionCache (VersionRefreshService) and probes
// completed-deployment endpoints; authenticated, no project scope (Rust
// version.rs, merged into protected_flat).
app.MapVersionEndpoints();

// POST /api/auth/login + GET /api/auth/profile — same response shapes the Rust
// dashboard serves. The policies (GlobalAdmin/Operator/Viewer, ProjectMember/
// Operator/Admin) are registered and available for other endpoints to opt into.
app.MapAuthEndpoints();

app.Run();

// Exposes the top-level-statement Program to WebApplicationFactory<Program>
// for integration tests (the standard minimal-API testing pattern).
public partial class Program { }
