using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class VmLifecycle
{
    public Guid EventId { get; set; }

    public string ProjectId { get; set; } = null!;

    public string ResourceType { get; set; } = null!;

    public Guid ResourceId { get; set; }

    public string? ResourceName { get; set; }

    public string Cloud { get; set; } = null!;

    public string? Region { get; set; }

    public string? VmSize { get; set; }

    public string? VmName { get; set; }

    public string? VmResourceId { get; set; }

    public Guid? CloudConnectionId { get; set; }

    public string? CloudAccountNameAtEvent { get; set; }

    public string? ProviderAccountId { get; set; }

    public string EventType { get; set; } = null!;

    public DateTime EventTime { get; set; }

    public Guid? TriggeredBy { get; set; }

    public string? Metadata { get; set; }

    public DateTime CreatedAt { get; set; }

    public virtual Project Project { get; set; } = null!;
}
