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

    /// <summary>
    /// V046: the tester's run envelope (client_network / client_geo /
    /// target_geo / client_load_before / client_load_after / clock_sync /
    /// client_info / server_info) as compact JSON, extracted from the final
    /// TestRun the agent parsed and relayed on run_finished. Null for runs
    /// executed by pre-envelope agents/testers — never backfilled.
    /// </summary>
    public string? ClientEnvelope { get; set; }

    public virtual Project Project { get; set; } = null!;

    public virtual TestConfig TestConfig { get; set; } = null!;

    public virtual ICollection<TestConfig> TestConfigs { get; set; } = new List<TestConfig>();

    public virtual ProjectTester? Tester { get; set; }
}
