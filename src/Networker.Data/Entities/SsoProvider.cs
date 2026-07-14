using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class SsoProvider
{
    public Guid ProviderId { get; set; }

    public string Name { get; set; } = null!;

    public string ProviderType { get; set; } = null!;

    public string ClientId { get; set; } = null!;

    public byte[] ClientSecretEnc { get; set; } = null!;

    public byte[] ClientSecretNonce { get; set; } = null!;

    public string? IssuerUrl { get; set; }

    public string? TenantId { get; set; }

    public string ExtraConfig { get; set; } = null!;

    public bool Enabled { get; set; }

    public short DisplayOrder { get; set; }

    public Guid? CreatedBy { get; set; }

    public DateTime CreatedAt { get; set; }

    public DateTime UpdatedAt { get; set; }
}
