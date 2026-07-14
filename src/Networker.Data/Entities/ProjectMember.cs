using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class ProjectMember
{
    public string ProjectId { get; set; } = null!;

    public Guid UserId { get; set; }

    public string Role { get; set; } = null!;

    public DateTime JoinedAt { get; set; }

    public Guid? InvitedBy { get; set; }

    public string Status { get; set; } = null!;

    public DateTime? InviteSentAt { get; set; }

    public virtual Project Project { get; set; } = null!;
}
