using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class CloudAccount
{
    public Guid AccountId { get; set; }

    public Guid? OwnerId { get; set; }

    public string Name { get; set; } = null!;

    public string Provider { get; set; } = null!;

    public byte[] CredentialsEnc { get; set; } = null!;

    public byte[] CredentialsNonce { get; set; } = null!;

    public string? RegionDefault { get; set; }

    public string Status { get; set; } = null!;

    public DateTime? LastValidated { get; set; }

    public string? ValidationError { get; set; }

    public DateTime CreatedAt { get; set; }

    public DateTime UpdatedAt { get; set; }

    public string ProjectId { get; set; } = null!;

    public virtual Project Project { get; set; } = null!;

    public virtual ICollection<ProjectTester> ProjectTesters { get; set; } = new List<ProjectTester>();
}
