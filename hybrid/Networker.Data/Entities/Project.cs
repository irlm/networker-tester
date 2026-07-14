using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class Project
{
    public string Name { get; set; } = null!;

    public string Slug { get; set; } = null!;

    public string? Description { get; set; }

    public Guid? CreatedBy { get; set; }

    public DateTime CreatedAt { get; set; }

    public DateTime UpdatedAt { get; set; }

    public string Settings { get; set; } = null!;

    public DateTime? DeletedAt { get; set; }

    public bool DeleteProtection { get; set; }

    public string ProjectId { get; set; } = null!;

    public virtual ICollection<Agent> Agents { get; set; } = new List<Agent>();

    public virtual ICollection<CloudAccount> CloudAccounts { get; set; } = new List<CloudAccount>();

    public virtual ICollection<ProjectTester> ProjectTesters { get; set; } = new List<ProjectTester>();

    public virtual ICollection<TestConfig> TestConfigs { get; set; } = new List<TestConfig>();

    public virtual ICollection<TestRun> TestRuns { get; set; } = new List<TestRun>();
}
