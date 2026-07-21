using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class TestConfig
{
    public Guid Id { get; set; }

    public string ProjectId { get; set; } = null!;

    public string Name { get; set; } = null!;

    public string? Description { get; set; }

    public string EndpointKind { get; set; } = null!;

    public string EndpointRef { get; set; } = null!;

    public string Workload { get; set; } = null!;

    public string? Methodology { get; set; }

    public Guid? CreatedBy { get; set; }

    public DateTime CreatedAt { get; set; }

    public DateTime UpdatedAt { get; set; }

    public Guid? BaselineRunId { get; set; }

    public int MaxDurationSecs { get; set; }

    /// <summary>
    /// AES-256-GCM ciphertext-with-tag of the LagHound SDK probe token
    /// (X-LagHound-Token) for an <c>sdkprobe</c> endpoint, or null. Encrypted
    /// with <c>Networker.Security.CredentialCipher</c> — the same scheme as
    /// <c>cloud_account.credentials_enc</c>. Paired with <see cref="TokenNonce"/>.
    /// Never serialized to a client (write-only; masked on read).
    /// </summary>
    public byte[]? TokenEnc { get; set; }

    /// <summary>The 12-byte GCM nonce for <see cref="TokenEnc"/>, or null.</summary>
    public byte[]? TokenNonce { get; set; }

    public virtual TestRun? BaselineRun { get; set; }

    public virtual Project Project { get; set; } = null!;

    public virtual ICollection<TestRun> TestRuns { get; set; } = new List<TestRun>();
}
