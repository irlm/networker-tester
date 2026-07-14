using Networker.Agent;

var builder = Host.CreateApplicationBuilder(args);

// Bind AgentOptions from config + environment (AGENT_ prefixed env vars map to
// the Agent section, e.g. AGENT_TARGET, AGENT_TESTERPATH, AGENT_MODES).
builder.Configuration.AddEnvironmentVariables(prefix: "AGENT_");
builder.Services.Configure<AgentOptions>(
    builder.Configuration.GetSection(AgentOptions.SectionName));
builder.Services.Configure<AgentOptions>(builder.Configuration);

builder.Services.AddSingleton<ProbeRunner>();
builder.Services.AddSingleton<IDashboardClient, NoOpDashboardClient>();
builder.Services.AddHostedService<AgentWorker>();

var host = builder.Build();
host.Run();
