using System.Collections.Concurrent;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Per-host cache of the endpoint's live capability self-report
/// (<see cref="TargetCapabilities.HostCapabilityReport"/>) for the
/// config-create gate. Same posture as <c>VersionEndpoints</c>'
/// <c>EndpointProbeCache</c>: the request path NEVER probes — it reads the last
/// result (if fresh) and, when the entry is missing or older than
/// <see cref="Ttl"/>, kicks off ONE background refresh per host (fire-and-
/// forget, in-flight dedup) so the <i>next</i> attempt is informed. A missing
/// or stale entry is reported as <c>null</c> = "no knowledge" and the caller
/// fails OPEN (kind / stack rules still apply).
///
/// <para>The deployment <c>/capabilities</c> route — the live probe the UI
/// runs when a target is picked — writes through here (<see cref="Store"/>),
/// so by the time the wizard submits the config the server usually already
/// holds a fresh report and the gate costs zero network I/O.</para>
/// </summary>
public sealed class LiveCapabilityCache
{
    /// <summary>How long a probe result counts as fresh for gating (short: an
    /// operator flipping a listener on should not be locked out for long).</summary>
    public static readonly TimeSpan DefaultTtl = TimeSpan.FromSeconds(90);

    private readonly Func<string, Task<TargetCapabilities.HostCapabilityReport>> _probe;
    private readonly TimeProvider _clock;
    private readonly ConcurrentDictionary<string, (TargetCapabilities.HostCapabilityReport Report, long At)> _cache =
        new(StringComparer.Ordinal);
    private readonly ConcurrentDictionary<string, byte> _inFlight = new(StringComparer.Ordinal);

    public LiveCapabilityCache()
        : this(TargetCapabilities.ProbeHostAsync, TimeProvider.System, DefaultTtl)
    {
    }

    /// <summary>Test seam: inject the prober / clock / TTL.</summary>
    public LiveCapabilityCache(
        Func<string, Task<TargetCapabilities.HostCapabilityReport>> probe,
        TimeProvider clock,
        TimeSpan ttl)
    {
        _probe = probe;
        _clock = clock;
        Ttl = ttl;
    }

    public TimeSpan Ttl { get; }

    /// <summary>A cached report plus its age.</summary>
    public readonly record struct Snapshot(TargetCapabilities.HostCapabilityReport Report, TimeSpan Age);

    /// <summary>
    /// The fresh report for <paramref name="host"/> — IMMEDIATELY, never
    /// awaiting a probe — or <c>null</c> when none is fresh (missing or older
    /// than <see cref="Ttl"/>), in which case a background refresh is triggered.
    /// </summary>
    public Snapshot? TryGetFresh(string host)
    {
        if (_cache.TryGetValue(host, out var entry))
        {
            var age = _clock.GetElapsedTime(entry.At);
            if (age < Ttl)
            {
                return new Snapshot(entry.Report, age);
            }
        }
        TriggerRefresh(host);
        return null;
    }

    /// <summary>The last stored report, fresh or not (diagnostics).</summary>
    public Snapshot? Peek(string host)
        => _cache.TryGetValue(host, out var e) ? new Snapshot(e.Report, _clock.GetElapsedTime(e.At)) : null;

    /// <summary>Write-through from a probe the caller already ran (the
    /// deployment /capabilities route).</summary>
    public void Store(TargetCapabilities.HostCapabilityReport report)
        => _cache[report.Host] = (report, _clock.GetTimestamp());

    /// <summary>Kick off one background probe for the host unless one is
    /// already running. Returns the probe task (tests await it); callers on the
    /// request path must NOT await it.</summary>
    public Task TriggerRefresh(string host)
    {
        if (!_inFlight.TryAdd(host, 0))
        {
            return Task.CompletedTask;
        }
        return Task.Run(async () =>
        {
            try
            {
                var report = await _probe(host).ConfigureAwait(false);
                Store(report);
            }
            catch
            {
                // Best-effort background refresh — never surfaces to a request.
            }
            finally
            {
                _inFlight.TryRemove(host, out _);
            }
        });
    }
}
