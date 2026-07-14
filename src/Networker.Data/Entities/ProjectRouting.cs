using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class ProjectRouting
{
    public string ProjectId { get; set; } = null!;

    public string HomeZone { get; set; } = null!;

    public string CurrentZone { get; set; } = null!;

    public DateTime? MigratedAt { get; set; }

    public Guid? MigratedBy { get; set; }
}
