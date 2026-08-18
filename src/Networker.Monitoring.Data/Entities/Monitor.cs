namespace Networker.Monitoring.Data.Entities;

public sealed class Monitor
{
    public Guid MonitorId { get; set; }
    public string ProjectId { get; set; } = string.Empty;
    public string Name { get; set; } = string.Empty;
    public string TargetUrl { get; set; } = string.Empty;
    public string Method { get; set; } = "GET";
    public string AssertionConfig { get; set; } = "{}";
    public int IntervalSeconds { get; set; } = 60;
    public int TimeoutMs { get; set; } = 10_000;
    public bool Enabled { get; set; } = true;
    public string Criticality { get; set; } = "reporting";
    public string RetentionPolicy { get; set; } = "{}";
    public DateTimeOffset NextCheckAt { get; set; }
    public string? LeaseOwner { get; set; }
    public DateTimeOffset? LeaseExpiresAt { get; set; }
    public DateTimeOffset CreatedAt { get; set; }
    public DateTimeOffset UpdatedAt { get; set; }
    public DateTimeOffset? DeletedAt { get; set; }

    public ICollection<MonitorLocationAssignment> LocationAssignments { get; set; } = [];
    public ICollection<MonitorCheck> Checks { get; set; } = [];
}
