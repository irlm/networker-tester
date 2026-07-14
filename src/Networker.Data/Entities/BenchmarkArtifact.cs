using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class BenchmarkArtifact
{
    public Guid Id { get; set; }

    public Guid TestRunId { get; set; }

    public string Environment { get; set; } = null!;

    public string Methodology { get; set; } = null!;

    public string Launches { get; set; } = null!;

    public string Cases { get; set; } = null!;

    public string? Samples { get; set; }

    public string Summaries { get; set; } = null!;

    public string DataQuality { get; set; } = null!;

    public DateTime CreatedAt { get; set; }

    public virtual TestRun TestRun { get; set; } = null!;
}
