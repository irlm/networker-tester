using Networker.Agent;

var builder = Host.CreateApplicationBuilder(args);

// Bind AgentOptions from config + environment (AGENT_ prefixed env vars map to
// the Agent section, e.g. AGENT_TARGET, AGENT_TESTERPATH, AGENT_MODES).
builder.Configuration.AddEnvironmentVariables(prefix: "AGENT_");
builder.Services.Configure<AgentOptions>(
    builder.Configuration.GetSection(AgentOptions.SectionName));
builder.Services.Configure<AgentOptions>(builder.Configuration);

builder.Services.AddSingleton<ProbeRunner>();

// Phase 2: real SignalR client to the control plane by default. Set
// AGENT_DASHBOARDURL=none to use the offline NoOp stub instead.
var dashUrl = builder.Configuration["DashboardUrl"] ?? builder.Configuration["Agent:DashboardUrl"];
if (string.Equals(dashUrl, "none", StringComparison.OrdinalIgnoreCase))
    builder.Services.AddSingleton<IDashboardClient, NoOpDashboardClient>();
else
    builder.Services.AddSingleton<IDashboardClient, SignalRDashboardClient>();

builder.Services.AddHostedService<AgentWorker>();

var host = builder.Build();
host.Run();
