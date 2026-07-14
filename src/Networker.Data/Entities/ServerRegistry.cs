using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class ServerRegistry
{
    public string ServerId { get; set; } = null!;

    public string ZoneCode { get; set; } = null!;

    public string Hostname { get; set; } = null!;

    public string Endpoint { get; set; } = null!;

    public string? InternalIp { get; set; }

    public string? DbUrl { get; set; }

    public string Status { get; set; } = null!;

    public DateTime? LastHealth { get; set; }

    public int Priority { get; set; }

    public DateTime CreatedAt { get; set; }

    public virtual SovereigntyZone Zone { get; set; } = null!;
}
