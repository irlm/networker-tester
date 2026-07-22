namespace Networker.ControlPlane.Auth;

/// <summary>
/// Per-IP brute-force throttle for the human auth surface (<c>POST
/// /api/auth/login</c>), the analogue of the agent-side
/// <see cref="Realtime.RawWs.AgentAuthLimiter"/> that guards <c>/ws/agent?key=</c>
/// (websec audit 2026-07, P1-2 — the agent path was throttled but the human login
/// was not, leaving an unbounded bcrypt-verify oracle). Kept as a SEPARATE bucket
/// from the agent limiter so an agent-key flood and a login-password flood from
/// the same source IP cannot cross-penalise each other; the sliding-window
/// mechanics are reused by composition rather than duplicated.
///
/// <para>Registered as a singleton in <c>AddNetworkerAuth</c>. In-memory and
/// process-local — same locality as the agent limiter, matching the single
/// control-plane-behind-nginx deployment shape. The window is 15 minutes (longer
/// than the agent limiter's 5) because a human retyping a password is slower than
/// an agent reconnect loop.</para>
/// </summary>
public sealed class LoginRateLimiter
{
    /// <summary>Failed logins from one IP within the window before it is blocked.</summary>
    public const int DefaultMaxFailures = 10;

    private readonly Realtime.RawWs.AgentAuthLimiter _inner;

    public LoginRateLimiter()
        : this(ResolveMaxFromEnv(), TimeSpan.FromMinutes(15))
    {
    }

    public LoginRateLimiter(int maxFailures, TimeSpan window)
        => _inner = new Realtime.RawWs.AgentAuthLimiter(maxFailures, window);

    /// <summary>The effective failure cap (test/observability hook).</summary>
    public int MaxFailures => _inner.MaxFailures;

    /// <summary><c>DASHBOARD_LOGIN_MAX_FAILURES</c>, default 10.</summary>
    public static int ResolveMaxFromEnv()
    {
        var raw = Environment.GetEnvironmentVariable("DASHBOARD_LOGIN_MAX_FAILURES");
        return int.TryParse(raw, out var v) && v > 0 ? v : DefaultMaxFailures;
    }

    /// <summary>True when this IP is at/over the failure cap within the window.</summary>
    public bool IsBlocked(string? ip) => _inner.IsBlocked(ip);

    /// <summary>Record one FAILED login from this IP.</summary>
    public void RecordFailure(string? ip) => _inner.RecordFailure(ip);

    /// <summary>Clear an IP's failure history on a SUCCESSFUL login.</summary>
    public void RecordSuccess(string? ip) => _inner.RecordSuccess(ip);
}
