using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class TestRun
{
    public Guid Id { get; set; }

    public Guid TestConfigId { get; set; }

    public string ProjectId { get; set; } = null!;

    public string Status { get; set; } = null!;

    public DateTime? StartedAt { get; set; }

    public DateTime? FinishedAt { get; set; }

    public int SuccessCount { get; set; }

    public int FailureCount { get; set; }

    public string? ErrorMessage { get; set; }

    public Guid? ArtifactId { get; set; }

    public Guid? TesterId { get; set; }

    public string? WorkerId { get; set; }

    public DateTime? LastHeartbeat { get; set; }

    public DateTime CreatedAt { get; set; }

    public Guid? ComparisonGroupId { get; set; }

    public Guid? ProvisioningDeploymentId { get; set; }

    public virtual Project Project { get; set; } = null!;

    public virtual TestConfig TestConfig { get; set; } = null!;

    public virtual ICollection<TestConfig> TestConfigs { get; set; } = new List<TestConfig>();

    public virtual ProjectTester? Tester { get; set; }
}
