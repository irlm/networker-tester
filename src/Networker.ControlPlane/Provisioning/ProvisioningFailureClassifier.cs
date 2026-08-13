namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// Classifies a failed auto-provision by its deploy log — today one class
/// matters: cloud CAPACITY QUOTA (Azure regional vCPU cores / public IPs).
/// Quota failures are transient by construction (every finished cell's
/// teardown frees capacity), so the orchestrator retries them instead of
/// failing the cell; everything else stays a real failure. The signatures are
/// matched against the raw Azure error text install.sh relays into the deploy
/// log (user-caught 2026-08-13: 13 matrix cells died on "exceeding approved
/// Total Regional Cores quota" surfaced as a bare "exited with code 1").
/// </summary>
public static class ProvisioningFailureClassifier
{
    // Case-insensitive substrings that identify a capacity-quota rejection.
    // Sources: Azure compute "OperationNotAllowed ... exceeding approved
    // Total Regional Cores quota" (the live 2026-08-13 log carried both the
    // phrase and a "Quota increase" portal link), and the network-side
    // PublicIPCountLimitReached (2026-08-01 incident, throttle doc).
    private static readonly string[] QuotaSignatures =
    [
        "exceeding approved",
        "QuotaExceeded",
        "quota increase",
        "PublicIPCountLimitReached",
        "quota limit",
    ];

    /// <summary>True when the failure text carries a capacity-quota signature.</summary>
    public static bool IsQuotaFailure(string? log, string? errorMessage)
    {
        return ContainsSignature(log) || ContainsSignature(errorMessage);
    }

    private static bool ContainsSignature(string? text)
    {
        if (string.IsNullOrEmpty(text))
        {
            return false;
        }
        foreach (var sig in QuotaSignatures)
        {
            if (text.Contains(sig, StringComparison.OrdinalIgnoreCase))
            {
                return true;
            }
        }
        return false;
    }
}
