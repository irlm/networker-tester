namespace Networker.Monitoring.Data.Entities;

public sealed class MonitorLocation
{
    public Guid LocationId { get; set; }
    public string Name { get; set; } = string.Empty;
    public string? Provider { get; set; }
    public string? Region { get; set; }
    public DateTimeOffset? LastHeartbeatAt { get; set; }
    public bool Enabled { get; set; } = true;
    public DateTimeOffset CreatedAt { get; set; }
    public DateTimeOffset UpdatedAt { get; set; }

    public ICollection<MonitorLocationAssignment> MonitorAssignments { get; set; } = [];
    public ICollection<MonitorCheck> Checks { get; set; } = [];
}
