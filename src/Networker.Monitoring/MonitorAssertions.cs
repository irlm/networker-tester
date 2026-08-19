using System.Text.Json.Serialization;

namespace Networker.Monitoring;

public sealed record MonitorAssertions
{
    [JsonPropertyName("status_min")]
    public int StatusMin { get; init; } = 200;

    [JsonPropertyName("status_max")]
    public int StatusMax { get; init; } = 399;

    [JsonPropertyName("warning_latency_ms")]
    public int? WarningLatencyMs { get; init; }

    [JsonPropertyName("critical_latency_ms")]
    public int? CriticalLatencyMs { get; init; }
}
