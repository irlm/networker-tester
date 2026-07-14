using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class SystemHealth
{
    public long Id { get; set; }

    public DateTime CheckedAt { get; set; }

    public string CheckName { get; set; } = null!;

    public string Status { get; set; } = null!;

    public string? Value { get; set; }

    public string? Message { get; set; }

    public string? Details { get; set; }
}
