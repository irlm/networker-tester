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

    /// <summary>Prefix of the error message <see cref="DeployRunner"/> writes when
    /// install.sh died to a signal (exit 143/137) — i.e. the control plane was
    /// restarted/killed mid-deploy (issue #764). Shared so the startup recovery
    /// pass and the orchestrator's retry arm match the EXACT message the runner
    /// produces, never a paraphrase that could drift.</summary>
    public const string InterruptedErrorPrefix = "Deployment interrupted (install.sh received SIG";

    /// <summary>True when the deployment's failure is the runner's
    /// interrupted-by-restart marker (<see cref="InterruptedErrorPrefix"/>).
    /// Interruption is transient by construction — the restart that caused it is
    /// already over by the time anyone can classify the failure — so callers
    /// retry instead of failing terminally (issue #764).</summary>
    public static bool IsInterruptedFailure(string? errorMessage)
    {
        return errorMessage is not null
               && errorMessage.StartsWith(InterruptedErrorPrefix, StringComparison.Ordinal);
    }

    /// <summary>Prefix of the error message the watchdog's stale-deploy sweep
    /// writes when it reaps a deployment whose install NEVER started (row still
    /// <c>pending</c> — <see cref="DeployRunner"/> flips to <c>running</c>
    /// immediately before spawning install.sh, so a budget-aged pending row
    /// means the deploy driver died before any install ran). Nothing was
    /// attempted, so nothing can have failed permanently — the linked run is
    /// retry-eligible (issue #817).</summary>
    public const string NeverStartedReapPrefix = "Deployment reaped before its install ever started";

    /// <summary>True when the failure is the watchdog's never-started reap
    /// marker (<see cref="NeverStartedReapPrefix"/>).</summary>
    public static bool IsNeverStartedReap(string? errorMessage)
    {
        return errorMessage is not null
               && errorMessage.StartsWith(NeverStartedReapPrefix, StringComparison.Ordinal);
    }

    /// <summary>True for any infrastructure-kill failure the orchestrator should
    /// retry instead of failing the run terminally: an install interrupted by a
    /// control-plane restart/shutdown (#764), or a watchdog reap of an install
    /// that never started (#817). Real install failures — credentials, bad
    /// config, a genuine in-budget timeout — never match and stay terminal.</summary>
    public static bool IsRetryableInfrastructureKill(string? errorMessage)
    {
        return IsInterruptedFailure(errorMessage) || IsNeverStartedReap(errorMessage);
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
