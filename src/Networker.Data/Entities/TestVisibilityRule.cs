using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class TestVisibilityRule
{
    public Guid RuleId { get; set; }

    public string ProjectId { get; set; } = null!;

    public Guid? UserId { get; set; }

    public string ResourceType { get; set; } = null!;

    public Guid ResourceId { get; set; }

    public Guid CreatedBy { get; set; }

    public DateTime CreatedAt { get; set; }

    public virtual Project Project { get; set; } = null!;
}
