using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class Agent
{
    public Guid AgentId { get; set; }

    public string Name { get; set; } = null!;

    public string? Region { get; set; }

    public string? Provider { get; set; }

    public string Status { get; set; } = null!;

    public string? Version { get; set; }

    public string? Os { get; set; }

    public string? Arch { get; set; }

    public DateTime? LastHeartbeat { get; set; }

    public DateTime RegisteredAt { get; set; }

    public string ApiKey { get; set; } = null!;

    /// <summary>
    /// Lowercase-hex SHA-256 of <see cref="ApiKey"/> (V040) — the column agent
    /// auth actually looks up. Nullable only for not-yet-migrated rows; V040
    /// backfills every existing row and minting writes both columns.
    /// </summary>
    public string? ApiKeyHash { get; set; }

    /// <summary>
    /// When the api-key expires (V044). Non-null + in the past → agent auth
    /// rejects the key (401, pre-upgrade). NULL = no expiry (back-compat for
    /// every fielded agent; rotation defaults to no-expiry).
    /// </summary>
    public DateTime? ApiKeyExpiresAt { get; set; }

    /// <summary>
    /// Last time the api-key successfully authenticated (V044) — the "last seen"
    /// signal the UI surfaces. Write-throttled to ~once per 5 min per agent so
    /// heartbeat/reconnect churn never causes a hot write.
    /// </summary>
    public DateTime? ApiKeyLastUsedAt { get; set; }

    /// <summary>Remote IP of the last successful api-key auth (V044, audit).</summary>
    public string? ApiKeyLastUsedIp { get; set; }

    public string? Tags { get; set; }

    public string ProjectId { get; set; } = null!;

    public Guid? TesterId { get; set; }

    public virtual Project Project { get; set; } = null!;

    public virtual ProjectTester? Tester { get; set; }
}
