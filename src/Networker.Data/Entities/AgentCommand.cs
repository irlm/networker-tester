using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class AgentCommand
{
    public Guid CommandId { get; set; }

    public Guid AgentId { get; set; }

    public Guid? ConfigId { get; set; }

    public string Verb { get; set; } = null!;

    public string Args { get; set; } = null!;

    public string Status { get; set; } = null!;

    public string? Result { get; set; }

    public string? ErrorMessage { get; set; }

    public Guid? CreatedBy { get; set; }

    public DateTime CreatedAt { get; set; }

    public DateTime? StartedAt { get; set; }

    public DateTime? FinishedAt { get; set; }

    public virtual Agent Agent { get; set; } = null!;
}
