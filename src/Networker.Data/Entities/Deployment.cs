using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class Deployment
{
    public Guid DeploymentId { get; set; }

    public string Name { get; set; } = null!;

    public string Status { get; set; } = null!;

    public string Config { get; set; } = null!;

    public string? ProviderSummary { get; set; }

    public Guid? CreatedBy { get; set; }

    public DateTime CreatedAt { get; set; }

    public DateTime? StartedAt { get; set; }

    public DateTime? FinishedAt { get; set; }

    public string? EndpointIps { get; set; }

    /// <summary>JSONB array parallel to <see cref="EndpointIps"/>: the DNS name of
    /// endpoint i (cloud FQDN / docker container name / lab alias) or null when
    /// the provider gave none. Dispatch prefers it over the IP (V050).</summary>
    public string? EndpointHosts { get; set; }

    public Guid? AgentId { get; set; }

    public string? ErrorMessage { get; set; }

    public string? Log { get; set; }

    public string ProjectId { get; set; } = null!;

    public Guid? CloudAccountId { get; set; }
}
