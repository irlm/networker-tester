using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class Agent
{
    public Guid AgentId { get; set; }

    public string Name { get; set; } = null!;

    public string? Region { get; set; }

    public string? Provider { get; set; }

    public string Status { get; set; } = null!;

    public string? Version { get; set; }

    public string? Os { get; set; }

    public string? Arch { get; set; }

    public DateTime? LastHeartbeat { get; set; }

    public DateTime RegisteredAt { get; set; }

    public string ApiKey { get; set; } = null!;

    public string? Tags { get; set; }

    public string ProjectId { get; set; } = null!;

    public Guid? TesterId { get; set; }

    public virtual Project Project { get; set; } = null!;

    public virtual ProjectTester? Tester { get; set; }
}
