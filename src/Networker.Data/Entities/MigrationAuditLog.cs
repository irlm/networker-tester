using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class MigrationAuditLog
{
    public Guid LogId { get; set; }

    public Guid RequestId { get; set; }

    public string Step { get; set; } = null!;

    public string Status { get; set; } = null!;

    public string? Details { get; set; }

    public string? Checksum { get; set; }

    public long? DurationMs { get; set; }

    public DateTime CreatedAt { get; set; }

    public virtual MigrationRequest Request { get; set; } = null!;
}
