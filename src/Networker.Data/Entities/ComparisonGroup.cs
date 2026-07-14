using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class ComparisonGroup
{
    public Guid Id { get; set; }

    public string ProjectId { get; set; } = null!;

    public string Name { get; set; } = null!;

    public string BaseWorkload { get; set; } = null!;

    public string? Methodology { get; set; }

    public string Cells { get; set; } = null!;

    public string Status { get; set; } = null!;

    public Guid? CreatedBy { get; set; }

    public DateTime CreatedAt { get; set; }

    public virtual Project Project { get; set; } = null!;

    public virtual ICollection<TestRun> TestRuns { get; set; } = new List<TestRun>();
}
