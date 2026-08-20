namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// The <b>pure</b> decision layer behind the SDK Endpoints page's create flow:
/// given what the catalog says a sample should be, what the project already
/// has deployed, and what the deployed thing actually answered when probed,
/// decide what the user should be offered — reuse, update, redeploy, create, or
/// wait.
///
/// <para>No DB, no HTTP, no clock: <see cref="SdkSampleEndpoints"/> gathers the
/// three inputs and this class turns them into a verdict, so the interesting
/// half is unit-testable without a host (SdkSamplePlanTests).</para>
///
/// <para><b>Cost is the tie-breaker.</b> Whenever a usable sample already
/// exists, the recommendation is <see cref="SdkSampleAction.Reuse"/> — a second
/// VM serving the identical sample is money spent for nothing. Only an outdated
/// sample earns <see cref="SdkSampleAction.Update"/> (in place, same VM), and
/// only a broken one earns <see cref="SdkSampleAction.Redeploy"/>.</para>
/// </summary>
public static class SdkSamplePlan
{
    /// <summary>What the project has for one language, honestly labelled.</summary>
    public enum State
    {
        /// <summary>No deployment in this project carries this sample.</summary>
        None,

        /// <summary>A deployment carrying this sample is still pending/running.</summary>
        Deploying,

        /// <summary>Reachable and running the catalog's current SDK version.</summary>
        Current,

        /// <summary>Reachable, but running an SDK version older than the catalog's.</summary>
        Outdated,

        /// <summary>Reachable, but the version could not be read (no stored
        /// token to authenticate the health probe, or a response without
        /// <c>sdk.version</c>). Usable, but never claimed as current.</summary>
        UnknownVersion,

        /// <summary>The deployment completed but the sample does not answer.</summary>
        Unhealthy,

        /// <summary>The deployment itself failed / was cancelled / torn down.</summary>
        Failed,
    }

    /// <summary>The single action the UI should offer for a language.</summary>
    public enum Action
    {
        /// <summary>Provision a server for this sample.</summary>
        Create,

        /// <summary>A deploy is in flight — offer nothing, wait for it.</summary>
        Wait,

        /// <summary>Register/keep the existing server. The cheap default.</summary>
        Reuse,

        /// <summary>Re-run the existing deployment in place to refresh the sample.</summary>
        Update,

        /// <summary>The existing server is broken — replace it.</summary>
        Redeploy,
    }

    /// <summary>
    /// What the project already has for one language, as read from the
    /// <c>deployment</c> row that declared the sample (plus the SDK endpoint
    /// registration pointing at it, when one exists).
    /// </summary>
    /// <param name="DeploymentId">The deployment that carries the sample.</param>
    /// <param name="DeploymentName">Its display name.</param>
    /// <param name="DeploymentStatus">Raw deployment status (pending/running/completed/failed/…).</param>
    /// <param name="Host">Host the sample serves on (DNS name preferred over ip); null before the deploy reported one.</param>
    /// <param name="Provider">Cloud provider of the host (azure/aws/gcp/docker/lan).</param>
    /// <param name="Region">Region/zone, when the config carried one.</param>
    /// <param name="VmSize">Normalised VM size (azure vm_size / aws instance_type / gcp machine_type).</param>
    /// <param name="Consolidated">True when this host carries more than one sample.</param>
    /// <param name="SampleCountOnHost">How many samples this host carries.</param>
    /// <param name="RegisteredEndpointId">The SDK endpoint (test config) registered against this sample, if any.</param>
    public sealed record Deployed(
        Guid DeploymentId,
        string DeploymentName,
        string DeploymentStatus,
        string? Host,
        string Provider,
        string? Region,
        string? VmSize,
        bool Consolidated,
        int SampleCountOnHost,
        Guid? RegisteredEndpointId);

    /// <summary>The live answer from <c>GET {prefix}/health</c> on a deployed
    /// sample. <paramref name="Reachable"/> false means it did not answer at
    /// all; a reachable probe with a null <paramref name="Version"/> means it
    /// answered but did not (or could not) report <c>sdk.version</c>.</summary>
    public sealed record Probe(bool Reachable, string? Version, string? Lang);

    /// <summary>The verdict for one language.</summary>
    /// <param name="Language">Catalog sample id.</param>
    /// <param name="CurrentVersion">The catalog's SDK version — "what the sample is now".</param>
    /// <param name="DeployedVersion">What the running sample reports, when it could be read.</param>
    /// <param name="StateOf">Honest state.</param>
    /// <param name="Recommended">The action the UI should default to.</param>
    /// <param name="Reason">One sentence, safe to show verbatim.</param>
    /// <param name="Reusable">Whether a create can skip provisioning for this language.</param>
    public sealed record Verdict(
        string Language,
        string CurrentVersion,
        string? DeployedVersion,
        State StateOf,
        Action Recommended,
        string Reason,
        bool Reusable);

    /// <summary>Deployment statuses that mean "still working on it".</summary>
    public static bool IsInFlight(string? status) =>
        status is "pending" or "running" or "provisioning";

    /// <summary>Deployment statuses that mean "there is nothing serving".</summary>
    public static bool IsDead(string? status) =>
        status is "failed" or "cancelled" or "torn_down";

    /// <summary>
    /// Decide the state + recommended action for one language.
    /// <paramref name="deployed"/> null ⇒ nothing exists; <paramref name="probe"/>
    /// null ⇒ the sample was never probed (in-flight or hostless deployments).
    /// </summary>
    public static Verdict Decide(
        SdkSampleCatalog.Sample sample,
        Deployed? deployed,
        Probe? probe)
    {
        var current = sample.SdkVersion;

        if (deployed is null)
        {
            return new Verdict(sample.Id, current, null, State.None, Action.Create,
                "Nothing deployed for this language yet.", Reusable: false);
        }

        if (IsInFlight(deployed.DeploymentStatus))
        {
            return new Verdict(sample.Id, current, null, State.Deploying, Action.Wait,
                $"Deployment {deployed.DeploymentName} is {deployed.DeploymentStatus} — wait for it to finish.",
                Reusable: false);
        }

        if (IsDead(deployed.DeploymentStatus))
        {
            return new Verdict(sample.Id, current, null, State.Failed, Action.Redeploy,
                $"The last deployment for this language ({deployed.DeploymentName}) is {deployed.DeploymentStatus} — nothing is serving.",
                Reusable: false);
        }

        if (string.IsNullOrWhiteSpace(deployed.Host))
        {
            return new Verdict(sample.Id, current, null, State.Unhealthy, Action.Redeploy,
                $"Deployment {deployed.DeploymentName} finished without reporting a host — the sample is unreachable.",
                Reusable: false);
        }

        if (probe is null || !probe.Reachable)
        {
            return new Verdict(sample.Id, current, null, State.Unhealthy, Action.Redeploy,
                $"{deployed.Host}:{sample.Port} did not answer /health — the sample is not serving.",
                Reusable: false);
        }

        if (string.IsNullOrWhiteSpace(probe.Version))
        {
            // Alive, but we cannot prove which sample it runs. Reusable —
            // refusing to reuse would cost a VM — but never labelled current.
            return new Verdict(sample.Id, current, null, State.UnknownVersion, Action.Reuse,
                deployed.RegisteredEndpointId is null
                    ? $"{deployed.Host}:{sample.Port} is up, but no stored token could read its version — reuse it or register a token to verify."
                    : $"{deployed.Host}:{sample.Port} is up but did not report an SDK version — reuse it, unverified.",
                Reusable: true);
        }

        var deployedVersion = probe.Version.Trim();
        if (VersionEndpoints.VersionNewer(current, deployedVersion))
        {
            return new Verdict(sample.Id, current, deployedVersion, State.Outdated, Action.Update,
                $"Deployed sample runs SDK {deployedVersion}; the current sample is {current}. Update in place — no new server.",
                Reusable: true);
        }

        // Equal, or the deployed one is AHEAD of this build's catalog (a host
        // deployed from a newer tree). Either way there is nothing to install,
        // so reuse — an "update" here would be a silent downgrade.
        var reason = VersionEndpoints.VersionNewer(deployedVersion, current)
            ? $"Deployed sample runs SDK {deployedVersion}, newer than this build's catalog ({current}) — reuse it; there is nothing to update."
            : $"Deployed sample runs the current SDK {current} — reuse it (no new server, no cost).";
        return new Verdict(sample.Id, current, deployedVersion, State.Current, Action.Reuse, reason, Reusable: true);
    }

    /// <summary>Wire name for a state (snake_case, stable across releases).</summary>
    public static string Wire(State s) => s switch
    {
        State.None => "none",
        State.Deploying => "deploying",
        State.Current => "current",
        State.Outdated => "outdated",
        State.UnknownVersion => "unknown_version",
        State.Unhealthy => "unhealthy",
        State.Failed => "failed",
        _ => "none",
    };

    /// <summary>Wire name for an action (snake_case, stable across releases).</summary>
    public static string Wire(Action a) => a switch
    {
        Action.Create => "create",
        Action.Wait => "wait",
        Action.Reuse => "reuse",
        Action.Update => "update",
        Action.Redeploy => "redeploy",
        _ => "create",
    };

    /// <summary>How a create request wants its servers laid out.</summary>
    public enum Shape
    {
        /// <summary>One server hosting every selected sample — the cheap shape.</summary>
        Consolidated,

        /// <summary>One server per language — isolation, N× the cost.</summary>
        Separated,
    }

    /// <summary>Parse the wire shape; null for anything else.</summary>
    public static Shape? ParseShape(string? raw) => raw?.Trim().ToLowerInvariant() switch
    {
        "consolidated" => Shape.Consolidated,
        "separated" => Shape.Separated,
        _ => null,
    };

    /// <summary>
    /// Split the requested languages into the ones a create must actually
    /// provision and the ones it can reuse. <paramref name="reuseExisting"/>
    /// false forces everything into <c>provision</c> (the explicit "give me a
    /// fresh server anyway" escape hatch); it never silently reuses.
    /// </summary>
    public static (List<string> Reuse, List<string> Provision) Split(
        IReadOnlyList<string> languages,
        IReadOnlyDictionary<string, Verdict> verdicts,
        bool reuseExisting)
    {
        var reuse = new List<string>();
        var provision = new List<string>();
        foreach (var lang in languages)
        {
            if (reuseExisting && verdicts.TryGetValue(lang, out var v) && v.Reusable)
            {
                reuse.Add(lang);
            }
            else
            {
                provision.Add(lang);
            }
        }
        return (reuse, provision);
    }

    /// <summary>
    /// How many servers a shape needs for <paramref name="provisionCount"/>
    /// languages: 1 for consolidated (0 when nothing needs provisioning), N for
    /// separated. This is the number the cost preview multiplies by.
    /// </summary>
    public static int ServerCount(Shape shape, int provisionCount) =>
        provisionCount == 0 ? 0 : shape == Shape.Consolidated ? 1 : provisionCount;
}
