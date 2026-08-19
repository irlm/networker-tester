namespace Networker.Monitoring.Data.Entities;

public sealed class MonitorLocationAssignment
{
    public Guid MonitorId { get; set; }
    public Guid LocationId { get; set; }
    public bool ParticipatesInIncidentQuorum { get; set; } = true;
    public DateTimeOffset CreatedAt { get; set; }

    public Monitor Monitor { get; set; } = null!;
    public MonitorLocation Location { get; set; } = null!;
}
