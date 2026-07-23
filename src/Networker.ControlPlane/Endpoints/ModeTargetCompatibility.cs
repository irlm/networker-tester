namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Server-side mode ↔ target compatibility gate — Phase 2 enforcement of the
/// capability model whose classification lives in <c>shared/modes.json</c>'s
/// <c>requires</c> field (served by <c>GET /api/modes</c>, guarded by
/// <c>ModesManifestTests</c>). Mirrors the frontend gate in
/// <c>dashboard/src/lib/mode-capabilities.ts</c>
/// (<c>unsupportedReason</c>/<c>isModeSupported</c>) so the API rejects, with
/// <b>422</b> at config-create, exactly the (mode, target) combinations the UI
/// already disables — defense-in-depth for API clients that bypass the wizards.
///
/// <para><b>endpoint.kind → TargetKind:</b> <c>network → url</c>,
/// <c>proxy → endpoint</c>, <c>runtime → sdk</c>.</para>
///
/// <para><b>Fail-open for <c>pending</c> (and any unknown kind).</b> A
/// <c>pending</c> endpoint is a provisioning request whose real capability
/// depends on the <c>proxy_stack</c> / <c>language</c> chosen in the Full Stack
/// and Application Benchmark wizards — the <i>same</i> <c>pending</c> kind
/// legitimately carries throughput (Full Stack) <i>and</i> <c>apibench</c>
/// (Application Benchmark). It is therefore not resolvable from kind alone and
/// is intentionally not gated here; those flows constrain modes UI-side and via
/// the per-language capability matrix. Enforcing here would reject valid
/// Application Benchmark configs.</para>
/// </summary>
public static class ModeTargetCompatibility
{
    /// <summary>
    /// Maps the <c>endpoint.kind</c> discriminator to the frontend
    /// <c>TargetKind</c>, or <c>null</c> for kinds we cannot resolve to a fixed
    /// capability at create time (<c>pending</c> / unknown) — those fail open.
    /// </summary>
    public static string? TargetKindFor(string? endpointKind) => endpointKind switch
    {
        "network" => "url",
        "proxy" => "endpoint",
        "runtime" => "sdk",
        _ => null,
    };

    // Mirror of unsupportedReason(): can a target of this kind ever run a mode
    // with this requirement? Keep in lockstep with mode-capabilities.ts.
    private static bool Supports(string requirement, string targetKind) => requirement switch
    {
        "any" => true,
        // A provisioned endpoint (and an SDK endpoint host) serves these; only a
        // raw URL cannot.
        "networker-endpoint" => targetKind != "url",
        "sdk-endpoint" => targetKind == "sdk",
        // The reference-API suite is its own test type; no fixed target kind runs
        // it (apibench rides the fail-open `pending` provisioning path instead).
        "reference-apis" => false,
        _ => true,
    };

    /// <summary>Human-readable reason a requirement can't be met by a target.</summary>
    public static string ReasonFor(string requirement) => requirement switch
    {
        "networker-endpoint" =>
            "needs a networker-endpoint target (throughput / UDP / page-load servers), not an arbitrary URL",
        "sdk-endpoint" =>
            "needs a customer LagHound SDK endpoint (Server-Timing) — use the SDK / Application flow",
        "reference-apis" =>
            "needs the application-benchmark reference APIs — use the Application Benchmark flow",
        _ => "is not supported by this target",
    };

    /// <summary>
    /// The subset of <paramref name="modes"/> that can only ever fail against
    /// <paramref name="endpointKind"/>, as (mode, requirement) pairs. Empty when
    /// the config is compatible, when <paramref name="modes"/> is empty, or when
    /// the kind is fail-open (<c>pending</c> / unknown).
    /// </summary>
    public static IReadOnlyList<(string Mode, string Requirement)> IncompatibleModes(
        IEnumerable<string>? modes, string? endpointKind)
    {
        var targetKind = TargetKindFor(endpointKind);
        if (targetKind is null || modes is null)
        {
            return [];
        }

        var bad = new List<(string, string)>();
        foreach (var mode in modes)
        {
            if (string.IsNullOrWhiteSpace(mode))
            {
                continue;
            }

            var requirement = PlatformEndpoints.RequirementOf(mode);
            if (!Supports(requirement, targetKind))
            {
                bad.Add((mode, requirement));
            }
        }

        return bad;
    }
}
