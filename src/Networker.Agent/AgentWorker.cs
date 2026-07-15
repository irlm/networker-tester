using System.Collections.Concurrent;

namespace Networker.Agent;

/// <summary>
/// The long-running agent daemon. Combines the Rust <c>main.rs</c> reconnect
/// loop, <c>ws_client</c> control-message dispatch, and <c>heartbeat</c> loop
/// into one hosted service.
///
/// Lifecycle (Rust parity):
///   * <c>while (!stopping)</c>: connect one raw WebSocket; on drop, log +
///     sleep <c>ReconnectDelaySeconds</c> (Rust: flat 5s) and reconnect. Ctrl-C
///     / host shutdown breaks the loop (Rust: <c>tokio::select!</c> on
///     <c>ctrl_c</c>).
///   * On connect, launch the heartbeat pump (30s) sending
///     <c>{"type":"heartbeat","load":null,"version":...}</c>.
///   * Dispatch each inbound <see cref="ControlMessage"/>:
///       - <c>welcome</c>            → log registration.
///       - <c>assign_run</c>         → spawn a <see cref="RunExecutor"/> task,
///         tracked in a per-run cancellation map (max 4 concurrent, matching
///         Rust's semaphore).
///       - <c>cancel_run</c>         → cancel + drop that run's token.
///       - <c>command</c>            → run the verb, stream result.
///       - <c>cancel</c>             → no-op (Rust: no in-flight command is
///         cancellable yet — logged only).
///       - <c>heartbeat_ping</c>     → trace only.
///       - <c>shutdown</c>           → cancel all runs (connection then drops).
///   * On disconnect: cancel all in-flight runs (Rust aborts every task).
/// </summary>
public sealed class AgentWorker(
    ILogger<AgentWorker> logger,
    RunExecutor runExecutor,
    CommandHandler commandHandler,
    AgentOptions options) : BackgroundService
{
    /// <summary>Max concurrent probe runs — Rust <c>MAX_CONCURRENT_RUNS = 4</c>.</summary>
    private const int MaxConcurrentRuns = 4;

    private static readonly string AgentVersion =
        typeof(AgentWorker).Assembly.GetName().Version?.ToString() ?? "0.0.0";

    private readonly RawWebSocketClient _client = new(logger);
    private readonly ConcurrentDictionary<Guid, RunHandle> _running = new();
    private readonly SemaphoreSlim _runSlots = new(MaxConcurrentRuns, MaxConcurrentRuns);

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        if (string.IsNullOrEmpty(options.ApiKey))
        {
            logger.LogCritical("AGENT_API_KEY (AGENT_APIKEY) is required — refusing to start");
            return;
        }

        logger.LogInformation(
            "Networker agent starting dashboard_url={DashboardUrl}", options.DashboardUrl);

        while (!stoppingToken.IsCancellationRequested)
        {
            try
            {
                await _client.RunOnceAsync(
                    options.DashboardUrl,
                    options.ApiKey,
                    DispatchControlAsync,
                    HeartbeatLoopAsync,
                    stoppingToken).ConfigureAwait(false);
                logger.LogInformation("WebSocket connection closed normally");
            }
            catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested)
            {
                break;
            }
            catch (Exception ex)
            {
                logger.LogError(ex, "WebSocket connection error");
            }
            finally
            {
                CancelAllRuns("disconnect");
            }

            if (stoppingToken.IsCancellationRequested)
                break;

            logger.LogInformation(
                "Reconnecting in {Delay} seconds...", options.ReconnectDelaySeconds);
            try
            {
                await Task.Delay(
                    TimeSpan.FromSeconds(options.ReconnectDelaySeconds), stoppingToken)
                    .ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                break;
            }
        }

        logger.LogInformation("Shutdown signal received");
    }

    // ── Heartbeat pump (30s) ─────────────────────────────────────────────────────
    private async Task HeartbeatLoopAsync(RawWebSocketClient.IFrameSink sink, CancellationToken token)
    {
        var interval = TimeSpan.FromSeconds(options.HeartbeatIntervalSeconds);
        using var timer = new PeriodicTimer(interval);
        try
        {
            while (await timer.WaitForNextTickAsync(token).ConfigureAwait(false))
            {
                if (!sink.TrySend(new HeartbeatMessage(Load: null, Version: AgentVersion)))
                    break; // channel closed → connection gone
            }
        }
        catch (OperationCanceledException)
        {
            // Connection tearing down.
        }
    }

    // ── Control-message dispatch (ws_client::handle_control_message parity) ───────
    private Task DispatchControlAsync(
        ControlMessage message, RawWebSocketClient.IFrameSink sink, CancellationToken token)
    {
        switch (message)
        {
            case WelcomeMessage w:
                logger.LogInformation(
                    "Registered with dashboard agent_id={AgentId} name={Name}", w.AgentId, w.AgentName);
                break;

            case AssignRunMessage assign:
                HandleAssignRun(assign, sink, token);
                break;

            case CancelRunMessage cancel:
                HandleCancelRun(cancel.RunId);
                break;

            case CommandMessage cmd:
                HandleCommand(cmd, sink);
                break;

            case CancelMessage c:
                // Rust: no in-flight command is cancellable yet — logged only.
                logger.LogDebug(
                    "Received Cancel for command {CommandId}; no in-flight commands are cancellable yet",
                    c.CommandId);
                break;

            case HeartbeatPingMessage ping:
                logger.LogTrace("HeartbeatPing from dashboard server_time={Now}", ping.Now);
                break;

            case ShutdownMessage:
                logger.LogInformation("Shutdown request from dashboard — aborting all runs");
                CancelAllRuns("shutdown");
                break;
        }

        return Task.CompletedTask;
    }

    private void HandleAssignRun(
        AssignRunMessage assign, RawWebSocketClient.IFrameSink sink, CancellationToken connToken)
    {
        Guid runId;
        try
        {
            runId = assign.Run.GetProperty("id").GetGuid();
        }
        catch (Exception ex)
        {
            logger.LogError(ex, "assign_run: could not read run.id — dropping frame");
            return;
        }

        logger.LogInformation("AssignRun received (v2) run_id={RunId}", runId);

        var runCts = CancellationTokenSource.CreateLinkedTokenSource(connToken);
        var handle = new RunHandle(runCts);
        if (!_running.TryAdd(runId, handle))
        {
            logger.LogWarning("Duplicate assign_run for {RunId} — ignoring", runId);
            runCts.Dispose();
            return;
        }

        var config = assign.Config.Clone();

        _ = Task.Run(async () =>
        {
            // Semaphore-gated concurrency (Rust: acquire a permit before running).
            await _runSlots.WaitAsync(runCts.Token).ConfigureAwait(false);
            try
            {
                await runExecutor.ExecuteAsync(runId, config, sink, runCts.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                // Cancelled via cancel_run / shutdown / disconnect — the executor
                // emits the cancelled terminal itself when it observes the token;
                // if the cancel fired before execution began there is nothing to
                // report (Rust aborts the task the same way).
            }
            catch (Exception ex)
            {
                logger.LogError(ex, "Run {RunId} failed unexpectedly", runId);
                sink.TrySend(new ErrorMessage(runId, $"agent run task failed: {ex.Message}"));
                sink.TrySend(new RunFinishedMessage(runId, "failed", null));
            }
            finally
            {
                _runSlots.Release();
                if (_running.TryRemove(runId, out var h))
                    h.Dispose();
            }
        }, CancellationToken.None);
    }

    private void HandleCancelRun(Guid runId)
    {
        logger.LogWarning("CancelRun received (v2) run_id={RunId}", runId);
        if (_running.TryRemove(runId, out var handle))
        {
            handle.Cancel();
            handle.Dispose();
        }
    }

    private void HandleCommand(CommandMessage cmd, RawWebSocketClient.IFrameSink sink)
    {
        _ = Task.Run(() =>
        {
            var result = commandHandler.Run(cmd, sink);
            sink.TrySend(result);
        }, CancellationToken.None);
    }

    private void CancelAllRuns(string reason)
    {
        foreach (var (runId, handle) in _running.ToArray())
        {
            logger.LogWarning("Aborting run {RunId} due to {Reason}", runId, reason);
            handle.Cancel();
            if (_running.TryRemove(runId, out var h))
                h.Dispose();
        }
    }

    public override void Dispose()
    {
        _runSlots.Dispose();
        base.Dispose();
    }

    /// <summary>Per-run handle: a cancellation source the dispatcher trips on
    /// cancel_run / shutdown / disconnect (analogue of the Rust cancel_tx +
    /// JoinHandle abort).</summary>
    private sealed class RunHandle(CancellationTokenSource cts) : IDisposable
    {
        public void Cancel()
        {
            try { cts.Cancel(); } catch (ObjectDisposedException) { /* already disposed */ }
        }

        public void Dispose() => cts.Dispose();
    }
}
