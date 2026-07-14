using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class CommandApproval
{
    public Guid ApprovalId { get; set; }

    public string ProjectId { get; set; } = null!;

    public Guid AgentId { get; set; }

    public string CommandType { get; set; } = null!;

    public string CommandDetail { get; set; } = null!;

    public string Status { get; set; } = null!;

    public Guid RequestedBy { get; set; }

    public Guid? DecidedBy { get; set; }

    public DateTime RequestedAt { get; set; }

    public DateTime? DecidedAt { get; set; }

    public DateTime ExpiresAt { get; set; }

    public string? Reason { get; set; }

    public virtual Project Project { get; set; } = null!;

    public virtual Agent Agent { get; set; } = null!;
}
