using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class ShareLink
{
    public Guid LinkId { get; set; }

    public string ProjectId { get; set; } = null!;

    public string TokenHash { get; set; } = null!;

    public string ResourceType { get; set; } = null!;

    public Guid? ResourceId { get; set; }

    public string? Label { get; set; }

    public DateTime ExpiresAt { get; set; }

    public Guid CreatedBy { get; set; }

    public DateTime CreatedAt { get; set; }

    public bool Revoked { get; set; }

    public int AccessCount { get; set; }

    public DateTime? LastAccessed { get; set; }

    public virtual Project Project { get; set; } = null!;
}
