using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class SystemConfig
{
    public string Key { get; set; } = null!;

    public string Value { get; set; } = null!;

    public Guid? UpdatedBy { get; set; }

    public DateTime UpdatedAt { get; set; }
}
