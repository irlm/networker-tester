using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class TestSchedule
{
    public Guid Id { get; set; }

    public Guid TestConfigId { get; set; }

    public string ProjectId { get; set; } = null!;

    public string CronExpr { get; set; } = null!;

    public string Timezone { get; set; } = null!;

    public bool Enabled { get; set; }

    public DateTime? LastFiredAt { get; set; }

    public Guid? LastRunId { get; set; }

    public DateTime? NextFireAt { get; set; }

    public Guid? CreatedBy { get; set; }

    public DateTime CreatedAt { get; set; }

    public virtual TestConfig TestConfig { get; set; } = null!;

    public virtual Project Project { get; set; } = null!;

    public virtual TestRun? LastRun { get; set; }
}
