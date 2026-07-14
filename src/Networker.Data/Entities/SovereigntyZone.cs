using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class SovereigntyZone
{
    public string Code { get; set; } = null!;

    public string? ParentCode { get; set; }

    public string Name { get; set; } = null!;

    public string Display { get; set; } = null!;

    public string? LegalNote { get; set; }

    public string? ComplianceLevel { get; set; }

    public string? FallbackZone { get; set; }

    public string AutoDetect { get; set; } = null!;

    public bool RequiresApproval { get; set; }

    public bool RequiresMfa { get; set; }

    public string Status { get; set; } = null!;

    public DateTime CreatedAt { get; set; }

    public virtual ICollection<ServerRegistry> ServerRegistries { get; set; } = new List<ServerRegistry>();
}
