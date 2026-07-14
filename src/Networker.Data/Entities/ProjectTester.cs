using System;
using System.Collections.Generic;
using System.Net;

namespace Networker.Data.Entities;

public partial class ProjectTester
{
    public Guid TesterId { get; set; }

    public string ProjectId { get; set; } = null!;

    public string Name { get; set; } = null!;

    public string Cloud { get; set; } = null!;

    public string Region { get; set; } = null!;

    public string VmSize { get; set; } = null!;

    public string? VmName { get; set; }

    public string? VmResourceId { get; set; }

    public IPAddress? PublicIp { get; set; }

    public string SshUser { get; set; } = null!;

    public string PowerState { get; set; } = null!;

    public string Allocation { get; set; } = null!;

    public string? StatusMessage { get; set; }

    public Guid? LockedByConfigId { get; set; }

    public string? InstallerVersion { get; set; }

    public DateTime? LastInstalledAt { get; set; }

    public bool AutoShutdownEnabled { get; set; }

    public short AutoShutdownLocalHour { get; set; }

    public DateTime? NextShutdownAt { get; set; }

    public short ShutdownDeferralCount { get; set; }

    public bool AutoProbeEnabled { get; set; }

    public DateTime? LastUsedAt { get; set; }

    public int? AvgBenchmarkDurationSeconds { get; set; }

    public int BenchmarkRunCount { get; set; }

    public Guid CreatedBy { get; set; }

    public DateTime CreatedAt { get; set; }

    public DateTime UpdatedAt { get; set; }

    public Guid? CloudConnectionId { get; set; }

    public string? RequestedOs { get; set; }

    public string? RequestedVariant { get; set; }

    public string? OsDistro { get; set; }

    public string? OsVersion { get; set; }

    public string? OsVariant { get; set; }

    public string? OsArch { get; set; }

    public string? OsKernel { get; set; }

    public Guid? CloudAccountId { get; set; }

    public virtual ICollection<Agent> Agents { get; set; } = new List<Agent>();

    public virtual CloudAccount? CloudAccount { get; set; }

    public virtual Project Project { get; set; } = null!;

    public virtual ICollection<TestRun> TestRuns { get; set; } = new List<TestRun>();
}
