using System.Reflection;
using System.Text.Json;
using Microsoft.AspNetCore.Http.Json;
using Microsoft.EntityFrameworkCore;
using Networker.Monitoring;
using Networker.Monitoring.Data;
using Networker.Monitoring.Data.Migrations;
using Networker.Monitoring.Endpoints;
using Networker.Monitoring.Probe;
using Networker.Monitoring.Scheduling;

var builder = WebApplication.CreateBuilder(args);

var connectionString = builder.Configuration.GetConnectionString("Monitoring")
    ?? builder.Configuration["MONITORING_DB_URL_NPGSQL"];
if (string.IsNullOrEmpty(connectionString))
{
    if (!builder.Environment.IsDevelopment())
    {
        throw new InvalidOperationException(
            "MONITORING_DB_URL_NPGSQL (or ConnectionStrings:Monitoring) is required outside Development. " +
            "Point it at the monitoring-owned PostgreSQL database — the service must not fall back to localhost in production.");
    }

    connectionString = "Host=127.0.0.1;Port=5432;Database=networker_monitoring;Username=networker;Password=networker";
}

var apiKeyValue = builder.Configuration["MONITORING_API_KEY"];
if (string.IsNullOrEmpty(apiKeyValue))
{
    if (!builder.Environment.IsDevelopment())
    {
        throw new InvalidOperationException(
            "MONITORING_API_KEY is required outside Development. Generate a strong independent key before deployment.");
    }

    apiKeyValue = "dev-insecure-monitoring-key-change-me";
}

builder.Services.AddSingleton(new MonitoringApiKey(apiKeyValue));
builder.Services.AddSingleton(TimeProvider.System);
builder.Services.AddDbContext<MonitoringDbContext>(options => options.UseNpgsql(connectionString));
builder.Services.AddHttpClient("monitor-probe", client =>
{
    client.Timeout = Timeout.InfiniteTimeSpan;
    client.DefaultRequestHeaders.UserAgent.ParseAdd("LagHound-Monitoring/1.0");
}).ConfigurePrimaryHttpMessageHandler(() => new HttpClientHandler
{
    // Redirect destinations require the same target-policy validation as the
    // original URL. PR 3 adds that policy; foundation checks never follow one.
    AllowAutoRedirect = false,
});
builder.Services.AddScoped<IMonitorProbeExecutor, HttpMonitorProbeExecutor>();
builder.Services.AddScoped<MonitorCheckRunner>();
builder.Services.AddHostedService<MonitorScheduler>();
builder.Services.Configure<JsonOptions>(options =>
{
    options.SerializerOptions.PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower;
    options.SerializerOptions.DictionaryKeyPolicy = JsonNamingPolicy.SnakeCaseLower;
});

var app = builder.Build();

if (builder.Configuration["MONITORING_RUN_MIGRATIONS"] != "0")
{
    var result = await MonitoringSchemaMigrator.MigrateAsync(connectionString);
    app.Logger.LogInformation(
        "Monitoring schema migrations: {Applied} applied, {Existing} already recorded",
        result.Applied.Count,
        result.AlreadyApplied.Count);
}

app.Use(async (context, next) =>
{
    context.Response.Headers.XContentTypeOptions = "nosniff";
    context.Response.Headers.XFrameOptions = "DENY";
    context.Response.Headers["Referrer-Policy"] = "no-referrer";
    await next();
});
app.UseMiddleware<MonitoringApiKeyMiddleware>();

app.MapMethods("/health", ["GET", "HEAD"], async (
    MonitoringDbContext db,
    CancellationToken cancellationToken) =>
{
    var databaseOk = false;
    try
    {
        databaseOk = await db.Database.CanConnectAsync(cancellationToken);
    }
    catch
    {
        // Health must remain a bounded status response even when PostgreSQL is unavailable.
    }

    var version = Assembly.GetExecutingAssembly()
        .GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion ?? "unknown";
    return Results.Json(
        new
        {
            status = databaseOk ? "ok" : "degraded",
            version,
            database = databaseOk ? "ok" : "error",
            scheduler = builder.Configuration["MONITORING_BACKGROUND_SERVICES"] == "1"
                ? "enabled"
                : "disabled",
        },
        statusCode: databaseOk
            ? StatusCodes.Status200OK
            : StatusCodes.Status503ServiceUnavailable);
});

app.MapMonitorEndpoints();
app.Run();

public partial class Program;
