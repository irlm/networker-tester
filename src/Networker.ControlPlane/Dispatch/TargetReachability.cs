namespace Networker.ControlPlane.Dispatch;

/// <summary>
/// Launch-time TCP reachability probe. A run launched against a stopped
/// target (e.g. an auto-shutdown deallocated VM whose DNS still resolves)
/// used to grind through every mode × run burning full per-attempt timeouts
/// with nothing to show — the E2E report 2026-07-30. A 3-second connect
/// probe at launch turns that into an instant, actionable error.
/// </summary>
internal static class TargetReachability
{
    /// <summary>Default probe budget — same as the provisioning readiness gate.</summary>
    internal static readonly TimeSpan ProbeTimeout = TimeSpan.FromSeconds(3);

    /// <summary>True iff a TCP connection to host:port completes within the
    /// timeout. Never throws (probe failure is an answer, not an error).</summary>
    internal static async Task<bool> TcpReachableAsync(
        string host, int port, TimeSpan timeout, CancellationToken ct)
    {
        try
        {
            using var client = new System.Net.Sockets.TcpClient();
            using var cts = CancellationTokenSource.CreateLinkedTokenSource(ct);
            cts.CancelAfter(timeout);
            await client.ConnectAsync(host, port, cts.Token).ConfigureAwait(false);
            return client.Connected;
        }
        catch
        {
            return false;
        }
    }
}
