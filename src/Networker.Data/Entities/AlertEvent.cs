using System;

namespace Networker.Data.Entities;

public partial class AlertEvent
{
    public Guid EventId { get; set; }

    public Guid RuleId { get; set; }

    /// <summary>The run whose evaluation caused the state transition.</summary>
    public Guid RunId { get; set; }

    public DateTime FiredAt { get; set; }

    /// <summary>'firing' | 'resolved'.</summary>
    public string State { get; set; } = null!;

    /// <summary>The metric value observed on the triggering run.</summary>
    public double? Value { get; set; }

    public string? Message { get; set; }

    /// <summary>'pending' → 'delivered' | 'failed: ...' | 'skipped: ...'.</summary>
    public string? DeliveryStatus { get; set; }

    public virtual AlertRule Rule { get; set; } = null!;

    public virtual TestRun Run { get; set; } = null!;
}
