namespace Networker.Agent;

/// <summary>
/// Runtime configuration for the long-running agent daemon. This is the C#
/// mirror of the Rust <c>AgentConfig</c> (crates/networker-agent/src/config.rs),
/// extended with the tester-binary path the Rust executor discovers on disk.
///
/// Env vars use the <c>AGENT_</c> prefix with NO underscores between words
/// (Program.cs binds <c>AddEnvironmentVariables(prefix: "AGENT_")</c> straight
/// onto this object): <c>AGENT_DASHBOARDURL</c>, <c>AGENT_APIKEY</c>,
/// <c>AGENT_TESTERPATH</c>, <c>AGENT_NAME</c>. The Rust agent reads
/// <c>AGENT_DASHBOARD_URL</c> / <c>AGENT_API_KEY</c>; both underscore and
/// no-underscore forms are accepted here (see <see cref="Normalize"/>) so an
/// operator's existing env carries over.
/// </summary>
public sealed class AgentOptions
{
    public const string SectionName = "Agent";

    /// <summary>
    /// WebSocket URL of the dashboard control plane. The agent dials
    /// <c>{DashboardUrl}?key={ApiKey}</c> as a raw WebSocket (Rust default
    /// <c>ws://localhost:3000/ws/agent</c>). Configure via
    /// <c>AGENT_DASHBOARDURL</c> (or <c>AGENT_DASHBOARD_URL</c>).
    /// </summary>
    public string DashboardUrl { get; set; } = "ws://localhost:3000/ws/agent";

    /// <summary>
    /// API key authenticating with the control plane (Rust: required
    /// <c>AGENT_API_KEY</c>). Configure via <c>AGENT_APIKEY</c> (or
    /// <c>AGENT_API_KEY</c>). Empty is rejected at startup, matching Rust's
    /// hard error.
    /// </summary>
    public string ApiKey { get; set; } = string.Empty;

    /// <summary>
    /// Path to the <c>networker-tester</c> binary. When empty the agent
    /// discovers it on disk exactly like the Rust executor's
    /// <c>find_tester_binary</c> (target/debug, target/release, cwd + up to 5
    /// parents, then PATH). Configure via <c>AGENT_TESTERPATH</c>.
    /// </summary>
    public string TesterPath { get; set; } = string.Empty;

    /// <summary>Name this agent reports (informational; the control plane assigns
    /// the canonical agent_id/name in its welcome frame).</summary>
    public string Name { get; set; } = "hybrid-agent-1";

    /// <summary>
    /// Reconnect back-off (seconds) between dropped connections. Rust sleeps a
    /// flat 5s in its main reconnect loop; matched here.
    /// </summary>
    public int ReconnectDelaySeconds { get; set; } = 5;

    /// <summary>Heartbeat interval (seconds). Rust: 30s (heartbeat.rs).</summary>
    public int HeartbeatIntervalSeconds { get; set; } = 30;

    /// <summary>
    /// Fold the Rust underscore env-var spellings onto this object. The DI
    /// binder handles <c>AGENT_DASHBOARDURL</c> etc.; this catches the
    /// operator-facing <c>AGENT_DASHBOARD_URL</c> / <c>AGENT_API_KEY</c> /
    /// <c>AGENT_TESTER_PATH</c> forms so an existing Rust-agent environment is
    /// honoured without change. Explicit no-underscore values (already bound)
    /// win over the underscore fallback.
    /// </summary>
    public void ApplyRustEnvFallbacks(IReadOnlyDictionary<string, string?> env)
    {
        string? Get(string k) => env.TryGetValue(k, out var v) && !string.IsNullOrEmpty(v) ? v : null;

        if (DashboardUrl == "ws://localhost:3000/ws/agent" && Get("AGENT_DASHBOARD_URL") is { } url)
            DashboardUrl = url;
        if (string.IsNullOrEmpty(ApiKey) && Get("AGENT_API_KEY") is { } key)
            ApiKey = key;
        if (string.IsNullOrEmpty(TesterPath) && Get("AGENT_TESTER_PATH") is { } tp)
            TesterPath = tp;
    }
}
