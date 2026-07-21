using System.Collections.Concurrent;

namespace Networker.ControlPlane.Realtime.RawWs;

/// <summary>
/// Per-IP brute-force mitigation for agent api-key auth. Agents present their
/// key at <c>/ws/agent?key=</c> / <c>/hub/agent?key=</c> BEFORE the WebSocket
/// upgrade; a flood of guesses from one source IP is throttled here so an
/// attacker cannot cheaply enumerate keys (even though the 48-char CSPRNG key
/// space is astronomically large, cheap short-circuiting + an audit warning is
/// defense in depth and keeps the auth DB lookup off the hot path under attack).
///
/// <para>Design mirrors the raw-WS <c>SlidingWindowRateLimiter</c>: a per-IP
/// sliding window of recent FAILED attempts. Successful auth clears the IP's
/// counter (a legitimate agent that mistyped once is not penalised). When an IP
/// exceeds <see cref="MaxFailures"/> failures within the window it is short-
/// circuited (the endpoint returns 429) until the window drains.</para>
///
/// <para>Registered as a singleton (in-memory, process-local) — the same
/// lifetime and locality as <c>AgentConnectionRegistry</c>. This is not a
/// distributed limiter; it bounds a single control-plane instance, which is the
/// deployment shape (one control plane behind nginx).</para>
/// </summary>
public sealed class AgentAuthLimiter
{
    /// <summary>Failed attempts from one IP within <see cref="Window"/> before it is blocked.</summary>
    public const int DefaultMaxFailures = 10;

    private readonly int _maxFailures;
    private readonly TimeSpan _window;
    private readonly ConcurrentDictionary<string, Queue<DateTimeOffset>> _failures = new(StringComparer.Ordinal);
    private readonly object _gate = new();

    public AgentAuthLimiter(int? maxFailures = null, TimeSpan? window = null)
    {
        _maxFailures = maxFailures ?? ResolveMaxFromEnv();
        _window = window ?? TimeSpan.FromMinutes(5);
    }

    /// <summary>The effective failure cap (test/observability hook).</summary>
    public int MaxFailures => _maxFailures;

    /// <summary><c>DASHBOARD_AGENT_AUTH_MAX_FAILURES</c>, default 10.</summary>
    public static int ResolveMaxFromEnv()
    {
        var raw = Environment.GetEnvironmentVariable("DASHBOARD_AGENT_AUTH_MAX_FAILURES");
        return int.TryParse(raw, out var v) && v > 0 ? v : DefaultMaxFailures;
    }

    /// <summary>
    /// True when this IP is currently blocked (already at/over the failure cap
    /// within the window). Read-only — does not itself record an attempt. A
    /// null/empty IP is never blocked (we cannot attribute it; the hash lookup
    /// still gates auth).
    /// </summary>
    public bool IsBlocked(string? ip)
    {
        if (string.IsNullOrEmpty(ip))
        {
            return false;
        }

        lock (_gate)
        {
            return CountRecentLocked(ip) >= _maxFailures;
        }
    }

    /// <summary>Record one FAILED auth attempt from this IP.</summary>
    public void RecordFailure(string? ip)
    {
        if (string.IsNullOrEmpty(ip))
        {
            return;
        }

        var now = DateTimeOffset.UtcNow;
        lock (_gate)
        {
            var q = _failures.GetOrAdd(ip, _ => new Queue<DateTimeOffset>());
            PruneLocked(q, now - _window);
            q.Enqueue(now);
        }
    }

    /// <summary>
    /// Clear an IP's failure history on SUCCESSFUL auth — a legitimate agent
    /// that fat-fingered a key once is not held against a later good key.
    /// </summary>
    public void RecordSuccess(string? ip)
    {
        if (string.IsNullOrEmpty(ip))
        {
            return;
        }

        lock (_gate)
        {
            _failures.TryRemove(ip, out _);
        }
    }

    private int CountRecentLocked(string ip)
    {
        if (!_failures.TryGetValue(ip, out var q))
        {
            return 0;
        }

        PruneLocked(q, DateTimeOffset.UtcNow - _window);
        return q.Count;
    }

    private static void PruneLocked(Queue<DateTimeOffset> q, DateTimeOffset cutoff)
    {
        while (q.Count > 0 && q.Peek() < cutoff)
        {
            q.Dequeue();
        }
    }
}
