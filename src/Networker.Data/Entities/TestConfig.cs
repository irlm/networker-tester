using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class TestConfig
{
    public Guid Id { get; set; }

    public string ProjectId { get; set; } = null!;

    public string Name { get; set; } = null!;

    public string? Description { get; set; }

    public string EndpointKind { get; set; } = null!;

    public string EndpointRef { get; set; } = null!;

    public string Workload { get; set; } = null!;

    public string? Methodology { get; set; }

    public Guid? CreatedBy { get; set; }

    public DateTime CreatedAt { get; set; }

    public DateTime UpdatedAt { get; set; }

    public Guid? BaselineRunId { get; set; }

    public int MaxDurationSecs { get; set; }

    public virtual TestRun? BaselineRun { get; set; }

    public virtual Project Project { get; set; } = null!;

    public virtual ICollection<TestRun> TestRuns { get; set; } = new List<TestRun>();
}
