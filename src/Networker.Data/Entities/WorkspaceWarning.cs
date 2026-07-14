using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class WorkspaceWarning
{
    public Guid WarningId { get; set; }

    public string ProjectId { get; set; } = null!;

    public string WarningType { get; set; } = null!;

    public DateTime SentAt { get; set; }

    public virtual Project Project { get; set; } = null!;
}
