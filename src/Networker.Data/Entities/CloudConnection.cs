using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class CloudConnection
{
    public Guid ConnectionId { get; set; }

    public string Name { get; set; } = null!;

    public string Provider { get; set; } = null!;

    public string Config { get; set; } = null!;

    public string Status { get; set; } = null!;

    public DateTime? LastValidated { get; set; }

    public string? ValidationError { get; set; }

    public Guid? CreatedBy { get; set; }

    public DateTime CreatedAt { get; set; }

    public DateTime UpdatedAt { get; set; }

    public string? ProjectId { get; set; }
}
