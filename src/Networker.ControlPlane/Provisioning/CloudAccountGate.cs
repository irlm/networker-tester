namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// Shared launch gate for cloud accounts that are not <c>active</c>. The
/// runner-create path 422s on a non-active account
/// (<c>TesterWriteEndpoints.Create</c>), but the comparison-group launch and
/// the provisioning orchestrator both provisioned against broken accounts
/// anyway — cells burned a full provision + readiness timeout before failing
/// with an unrelated cloud error (#791 / #793 P1-1). Both now fail fast with
/// this message, which includes the account's own <c>validation_error</c> so
/// the user sees the credentials problem, not the downstream symptom.
/// </summary>
public static class CloudAccountGate
{
    /// <summary>
    /// Null when the account is <c>active</c>; otherwise the launch-blocking
    /// reason: <c>cloud account '&lt;name&gt;' is in &lt;status&gt; state[: &lt;validation_error&gt;]</c>.
    /// </summary>
    public static string? NotActiveReason(string name, string status, string? validationError)
    {
        if (status == "active")
        {
            return null;
        }
        var reason = $"cloud account '{name}' is in {status} state";
        return string.IsNullOrWhiteSpace(validationError) ? reason : $"{reason}: {validationError}";
    }
}
