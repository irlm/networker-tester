using Microsoft.AspNetCore.SignalR.Client;
using Microsoft.Extensions.Options;
using Networker.Contracts;

namespace Networker.Agent;

/// <summary>
/// Live control-plane transport over SignalR — the Phase 2 replacement for
/// <see cref="NoOpDashboardClient"/>. Connects to <c>{DashboardUrl}/ws/agent</c>
/// and invokes the hub's <c>ReportResult</c>/<c>Heartbeat</c> methods.
///
/// This is the whole point of the hybrid seam realised in C#: the Rust agent
/// hand-rolled a tokio-tungstenite client with a manual reconnect loop; here
/// <c>WithAutomaticReconnect()</c> and lazy connect are framework features.
/// </summary>
public sealed class SignalRDashboardClient : IDashboardClient, IAsyncDisposable
{
    private readonly ILogger<SignalRDashboardClient> _logger;
    private readonly AgentOptions _options;
    private readonly HubConnection _connection;
    private readonly SemaphoreSlim _connectGate = new(1, 1);

    public SignalRDashboardClient(ILogger<SignalRDashboardClient> logger, IOptions<AgentOptions> options)
    {
        _logger = logger;
        _options = options.Value;
        var hubUrl = $"{_options.DashboardUrl.TrimEnd('/')}/ws/agent";
        _connection = new HubConnectionBuilder()
            .WithUrl(hubUrl)
            .WithAutomaticReconnect()
            .Build();
        _logger.LogInformation("SignalR dashboard client targeting {HubUrl}", hubUrl);
    }

    private async Task EnsureConnectedAsync(CancellationToken ct)
    {
        if (_connection.State == HubConnectionState.Connected) return;
        await _connectGate.WaitAsync(ct).ConfigureAwait(false);
        try
        {
            switch (_connection.State)
            {
                case HubConnectionState.Connected:
                    return;
                case HubConnectionState.Disconnected:
                    await _connection.StartAsync(ct).ConfigureAwait(false);
                    return;
                default:
                    // Connecting / Reconnecting (WithAutomaticReconnect is
                    // mid-flight). Proceeding here would make InvokeAsync throw
                    // "connection is not active" — poll until it settles.
                    await WaitForConnectedAsync(ct).ConfigureAwait(false);
                    return;
            }
        }
        finally
        {
            _connectGate.Release();
        }
    }

    private async Task WaitForConnectedAsync(CancellationToken ct)
    {
        // Bounded wait: the automatic reconnector is working; give it a window
        // to reach Connected before surfacing the failure to the caller.
        for (var i = 0; i < 30; i++)
        {
            if (_connection.State == HubConnectionState.Connected) return;
            if (_connection.State == HubConnectionState.Disconnected)
            {
                await _connection.StartAsync(ct).ConfigureAwait(false);
                return;
            }
            await Task.Delay(TimeSpan.FromMilliseconds(500), ct).ConfigureAwait(false);
        }
        throw new InvalidOperationException(
            "SignalR connection did not reach Connected within the reconnect window");
    }

    public async Task ReportResultAsync(ProbeRunResult result, CancellationToken cancellationToken = default)
    {
        await EnsureConnectedAsync(cancellationToken).ConfigureAwait(false);
        await _connection.InvokeAsync("ReportResult", result, cancellationToken).ConfigureAwait(false);
        _logger.LogInformation(
            "Reported run {RunId} to control plane over SignalR ({Count} attempts)",
            result.RunId, result.Attempts.Count);
    }

    public async Task HeartbeatAsync(CancellationToken cancellationToken = default)
    {
        await EnsureConnectedAsync(cancellationToken).ConfigureAwait(false);
        await _connection.InvokeAsync("Heartbeat", _options.Name, cancellationToken).ConfigureAwait(false);
    }

    public async ValueTask DisposeAsync() => await _connection.DisposeAsync().ConfigureAwait(false);
}
