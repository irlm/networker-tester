using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class BenchmarkVmCatalog
{
    public Guid VmId { get; set; }

    public string ProjectId { get; set; } = null!;

    public string Name { get; set; } = null!;

    public string Cloud { get; set; } = null!;

    public string Region { get; set; } = null!;

    public string Ip { get; set; } = null!;

    public string SshUser { get; set; } = null!;

    public string Languages { get; set; } = null!;

    public string? VmSize { get; set; }

    public string Status { get; set; } = null!;

    public DateTime? LastHealthCheck { get; set; }

    public Guid? CreatedBy { get; set; }

    public DateTime CreatedAt { get; set; }

    public virtual Project Project { get; set; } = null!;
}
