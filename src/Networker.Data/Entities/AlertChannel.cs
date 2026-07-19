using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class AlertChannel
{
    public Guid ChannelId { get; set; }

    public string ProjectId { get; set; } = null!;

    /// <summary>'webhook' or 'email'.</summary>
    public string Kind { get; set; } = null!;

    public string Name { get; set; } = null!;

    /// <summary>
    /// JSONB, kind-specific: webhook = <c>{"url": "...", "secret": "..."}</c>
    /// (secret optional — enables the HMAC signature header);
    /// email = <c>{"to": ["a@example.com", ...]}</c>.
    /// </summary>
    public string Config { get; set; } = null!;

    public bool Enabled { get; set; }

    public DateTime CreatedAt { get; set; }

    public virtual Project Project { get; set; } = null!;

    public virtual ICollection<AlertRule> AlertRules { get; set; } = new List<AlertRule>();
}
