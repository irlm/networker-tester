namespace Networker.Monitoring.Data.Entities;

public sealed class MonitorCheck
{
    public Guid CheckId { get; set; }
    public Guid MonitorId { get; set; }
    public Guid LocationId { get; set; }
    public DateTimeOffset ScheduledAt { get; set; }
    public DateTimeOffset StartedAt { get; set; }
    public DateTimeOffset FinishedAt { get; set; }
    public string Outcome { get; set; } = "unknown";
    public string? FailureKind { get; set; }
    public int? StatusCode { get; set; }
    public int? DnsMs { get; set; }
    public int? ConnectMs { get; set; }
    public int? TlsMs { get; set; }
    public int? TtfbMs { get; set; }
    public int? TotalMs { get; set; }
    public string AssertionResults { get; set; } = "{}";
    public string? ResponseDigest { get; set; }
    public string? ErrorSummary { get; set; }
    public DateTimeOffset CreatedAt { get; set; }

    public Monitor Monitor { get; set; } = null!;
    public MonitorLocation Location { get; set; } = null!;
}
