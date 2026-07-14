using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class MigrationRequest
{
    public Guid RequestId { get; set; }

    public string ProjectId { get; set; } = null!;

    public string FromZone { get; set; } = null!;

    public string ToZone { get; set; } = null!;

    public string Reason { get; set; } = null!;

    public Guid RequestedBy { get; set; }

    public Guid? ApprovedBy { get; set; }

    public string Status { get; set; } = null!;

    public DateTime? ScheduledAt { get; set; }

    public DateTime? StartedAt { get; set; }

    public DateTime? CompletedAt { get; set; }

    public long? DataSizeMb { get; set; }

    public string? ErrorMessage { get; set; }

    public DateTime CreatedAt { get; set; }

    public virtual ICollection<MigrationAuditLog> MigrationAuditLogs { get; set; } = new List<MigrationAuditLog>();
}
