using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class AlertRule
{
    public Guid RuleId { get; set; }

    public string ProjectId { get; set; } = null!;

    /// <summary>NULL = the rule applies to every test_config in the project.</summary>
    public Guid? TestConfigId { get; set; }

    /// <summary>'p95_ms' | 'mean_ms' | 'error_rate' | 'success_rate'.</summary>
    public string Metric { get; set; } = null!;

    /// <summary>'gt' | 'lt'.</summary>
    public string Comparator { get; set; } = null!;

    public double Threshold { get; set; }

    /// <summary>Consecutive terminal runs that must breach before firing.</summary>
    public int WindowRuns { get; set; }

    public bool Enabled { get; set; }

    public Guid ChannelId { get; set; }

    public Guid? CreatedBy { get; set; }

    public DateTime CreatedAt { get; set; }

    public virtual Project Project { get; set; } = null!;

    public virtual TestConfig? TestConfig { get; set; }

    public virtual AlertChannel Channel { get; set; } = null!;

    public virtual ICollection<AlertEvent> AlertEvents { get; set; } = new List<AlertEvent>();
}
