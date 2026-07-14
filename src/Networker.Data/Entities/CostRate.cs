using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class CostRate
{
    public Guid CostRateId { get; set; }

    public string Cloud { get; set; } = null!;

    public string VmSize { get; set; } = null!;

    public string? Region { get; set; }

    public decimal RatePerHourUsd { get; set; }

    public DateTime EffectiveFrom { get; set; }

    public DateTime? EffectiveTo { get; set; }

    public string Source { get; set; } = null!;

    public DateTime CreatedAt { get; set; }
}
