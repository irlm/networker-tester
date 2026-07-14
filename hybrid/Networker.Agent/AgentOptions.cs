namespace Networker.Agent;

/// <summary>Runtime configuration for the agent skeleton.</summary>
public sealed class AgentOptions
{
    public const string SectionName = "Agent";

    /// <summary>
    /// Path to the `networker-tester` binary. When not overridden, the agent
    /// assumes it is on PATH. Configure via <c>AGENT_TESTER_PATH</c> env var or
    /// <c>Agent:TesterPath</c>.
    /// </summary>
    public string TesterPath { get; set; } = "networker-tester";

    /// <summary>Target URL the one-shot startup probe runs against.</summary>
    public string Target { get; set; } = "https://www.cloudflare.com";

    /// <summary>Probe modes passed to the tester (comma-separated).</summary>
    public string Modes { get; set; } = "http1";

    /// <summary>Per-run timeout in seconds handed to the tester.</summary>
    public int TimeoutSeconds { get; set; } = 30;
}
