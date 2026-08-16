namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Server-side mode ↔ target compatibility gate — Phase 2 enforcement of the
/// capability model whose classification lives in <c>shared/modes.json</c>'s
/// <c>requires</c> field (served by <c>GET /api/modes</c>, guarded by
/// <c>ModesManifestTests</c>) plus the per-stack HTTP/3 capability in
/// <c>shared/http-stacks.json</c> (<see cref="HttpStackCatalog"/>, served by
/// <c>GET /api/http-stacks</c>). Mirrors the frontend gate in
/// <c>dashboard/src/lib/mode-capabilities.ts</c>
/// (<c>unsupportedReason</c>/<c>isModeSupported</c>) so the API rejects, with
/// <b>422</b> at config-create, exactly the (mode, target) combinations the UI
/// already disables — defense-in-depth for API clients that bypass the wizards.
///
/// <para><b>endpoint.kind → TargetKind:</b> <c>network → url</c>,
/// <c>proxy → endpoint</c>, <c>runtime → sdk</c>.</para>
///
/// <para><b>Two rules.</b> (1) <i>Requirement by kind</i>: throughput / UDP /
/// page-load need a networker-endpoint (not a raw URL), <c>sdkprobe</c> needs
/// an SDK endpoint, <c>apibench</c> needs the reference APIs. (2) <i>HTTP/3 by
/// stack</i>: the h3 modes (<c>http3</c>, <c>pageload3</c>, <c>browser3</c>,
/// <c>download3</c>, <c>upload3</c> — <c>h3_modes</c> in the manifest) can only
/// fail through a proxy stack the installer configures without QUIC
/// (apache / haproxy / traefik: the lab measured them 0/N). The stack comes
/// from the resolved target descriptor — <c>endpoint.proxy_stack</c>, the
/// deployment's <c>http_stacks[0]</c> (the listener the dispatcher resolves
/// <c>proxy</c> endpoints to), or <c>pending.proxy_stack</c>.</para>
///
/// <para><b>Fail-open for <c>pending</c> (and any unknown kind) — rule 1 only.</b>
/// A <c>pending</c> endpoint is a provisioning request whose real capability
/// depends on the <c>proxy_stack</c> / <c>language</c> chosen in the Full Stack
/// and Application Benchmark wizards — the <i>same</i> <c>pending</c> kind
/// legitimately carries throughput (Full Stack) <i>and</i> <c>apibench</c>
/// (Application Benchmark). It is therefore not resolvable from kind alone and
/// is intentionally not gated by rule 1; those flows constrain modes UI-side
/// and via the per-language capability matrix. Rule 2 IS decidable for pending
/// (the stack is right there) and is applied. Unknown stacks fail open.</para>
/// </summary>
public static class ModeTargetCompatibility
{
    /// <summary>Requirement id reported for an HTTP/3-by-stack rejection.</summary>
    public const string H3Requirement = "h3";

    /// <summary>One rejected (mode, target) pair with its human-readable reason.</summary>
    public sealed record Incompatibility(string Mode, string Requirement, string Reason);

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
        H3Requirement =>
            "needs HTTP/3 (QUIC) on the target, which this proxy stack does not serve (see shared/http-stacks.json)",
        _ => "is not supported by this target",
    };

    /// <summary>The reason an h3 mode is rejected on <paramref name="stack"/>.</summary>
    public static string H3ReasonFor(string stack) =>
        $"needs HTTP/3 (QUIC): {stack} has no HTTP/3 (see shared/http-stacks.json)";

    /// <summary>
    /// The subset of <paramref name="modes"/> that can only ever fail against
    /// the target, as (mode, requirement, reason) triples. Empty when the config
    /// is compatible, when <paramref name="modes"/> is empty, or when nothing is
    /// decidable (kind fail-open AND no known stack).
    /// </summary>
    /// <param name="modes">workload.modes.</param>
    /// <param name="endpointKind">endpoint.kind (<c>network</c> / <c>proxy</c> / <c>runtime</c> / <c>pending</c>).</param>
    /// <param name="proxyStack">The proxy stack the target resolves to (an id
    /// from <c>shared/http-stacks.json</c>), or null when unknown / not a proxy
    /// target. Unknown ids fail open.</param>
    public static IReadOnlyList<Incompatibility> IncompatibleModes(
        IEnumerable<string>? modes, string? endpointKind, string? proxyStack = null)
    {
        if (modes is null)
        {
            return [];
        }

        var targetKind = TargetKindFor(endpointKind);
        var stack = HttpStackCatalog.Find(proxyStack);
        var stackLacksH3 = stack is { H3: false };
        if (targetKind is null && !stackLacksH3)
        {
            return [];
        }

        var bad = new List<Incompatibility>();
        foreach (var mode in modes)
        {
            if (string.IsNullOrWhiteSpace(mode))
            {
                continue;
            }

            if (targetKind is not null)
            {
                var requirement = PlatformEndpoints.RequirementOf(mode);
                if (!Supports(requirement, targetKind))
                {
                    bad.Add(new Incompatibility(mode, requirement, ReasonFor(requirement)));
                    continue;
                }
            }

            if (stackLacksH3 && HttpStackCatalog.IsH3Mode(mode))
            {
                bad.Add(new Incompatibility(mode, H3Requirement, H3ReasonFor(stack!.Id)));
            }
        }

        return bad;
    }

    /// <summary>
    /// Split <paramref name="modes"/> into the ones a proxy stack can serve and
    /// the HTTP/3 modes it cannot — the per-cell drop for comparison groups /
    /// matrix runs (an apache cell simply does not run h3 modes; the group
    /// keeps running). Unknown / null stacks drop nothing.
    /// </summary>
    public static (IReadOnlyList<string> Kept, IReadOnlyList<string> Dropped) SplitH3ModesForStack(
        IEnumerable<string> modes, string? proxyStack)
    {
        var kept = new List<string>();
        var dropped = new List<string>();
        var lacksH3 = HttpStackCatalog.HasH3(proxyStack) == false;
        foreach (var m in modes)
        {
            if (lacksH3 && HttpStackCatalog.IsH3Mode(m))
            {
                dropped.Add(m);
            }
            else
            {
                kept.Add(m);
            }
        }
        return (kept, dropped);
    }
}
