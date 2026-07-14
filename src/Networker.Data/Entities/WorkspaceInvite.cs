using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class WorkspaceInvite
{
    public Guid InviteId { get; set; }

    public string ProjectId { get; set; } = null!;

    public string Email { get; set; } = null!;

    public string Role { get; set; } = null!;

    public string TokenHash { get; set; } = null!;

    public string Status { get; set; } = null!;

    public Guid InvitedBy { get; set; }

    public DateTime CreatedAt { get; set; }

    public DateTime ExpiresAt { get; set; }

    public DateTime? AcceptedAt { get; set; }

    public Guid? AcceptedBy { get; set; }

    public virtual Project Project { get; set; } = null!;
}
