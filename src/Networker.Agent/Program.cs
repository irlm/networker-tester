using Networker.Agent;

var builder = Host.CreateApplicationBuilder(args);

// Bind AgentOptions from config + environment. AGENT_-prefixed env vars map onto
// the option object directly (no underscores between words):
//   AGENT_DASHBOARDURL, AGENT_APIKEY, AGENT_TESTERPATH, AGENT_NAME.
// The [Agent] appsettings section is bound too so a config file works locally.
builder.Configuration.AddEnvironmentVariables(prefix: "AGENT_");
builder.Services.Configure<AgentOptions>(
    builder.Configuration.GetSection(AgentOptions.SectionName));
builder.Services.Configure<AgentOptions>(builder.Configuration);

// Resolve the bound options + apply the Rust underscore env spellings
// (AGENT_DASHBOARD_URL / AGENT_API_KEY / AGENT_TESTER_PATH) as a fallback so an
// existing Rust-agent environment carries over unchanged.
builder.Services.AddSingleton(sp =>
{
    var opts = new AgentOptions();
    builder.Configuration.GetSection(AgentOptions.SectionName).Bind(opts);
    builder.Configuration.Bind(opts);

    var env = new Dictionary<string, string?>
    {
        ["AGENT_DASHBOARD_URL"] = Environment.GetEnvironmentVariable("AGENT_DASHBOARD_URL"),
        ["AGENT_API_KEY"] = Environment.GetEnvironmentVariable("AGENT_API_KEY"),
        ["AGENT_TESTER_PATH"] = Environment.GetEnvironmentVariable("AGENT_TESTER_PATH"),
    };
    opts.ApplyRustEnvFallbacks(env);
    return opts;
});

builder.Services.AddSingleton<RunExecutor>();
builder.Services.AddSingleton<CommandHandler>();
builder.Services.AddHostedService<AgentWorker>();

var host = builder.Build();
host.Run();
