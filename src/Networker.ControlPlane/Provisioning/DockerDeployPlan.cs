using System.Text.Json;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// The docker-provider reading of a deploy.json: which endpoint entries carry
/// <c>provider: "docker"</c> and what target container each one becomes. The
/// deploy runner takes this branch instead of shelling <c>install.sh --deploy</c>
/// (there is no VM to SSH into — the target image already has the endpoint +
/// the proxy stack that <c>install.sh --setup-stack</c> installed at image build).
///
/// <para>Rules (pure, unit-tested):</para>
/// <list type="bullet">
///   <item>An endpoint is docker iff <c>provider == "docker"</c>.</item>
///   <item>Docker endpoints cannot be mixed with cloud/lan endpoints in one
///     deployment (the runner is either all-docker or all-install.sh).</item>
///   <item>Exactly one <c>http_stacks</c> entry per docker endpoint (one
///     container = one ip = one stack, matching how the proxy resolver reads
///     <c>endpoint_ips[0]</c> + stack port); none = the bare rust endpoint.</item>
///   <item><c>languages</c> (reference-API servers) are not provided by the
///     target images → rejected.</item>
/// </list>
/// </summary>
public sealed record DockerDeployPlan(IReadOnlyList<DockerDeployPlan.Endpoint> Endpoints)
{
    /// <summary>One target container to start.</summary>
    public sealed record Endpoint(int Index, string Label, string? Stack);

    /// <summary>
    /// Parse <paramref name="deployJson"/>. Returns <c>null</c> plan when no
    /// endpoint is docker (the caller runs the install.sh path). When a docker
    /// endpoint is present but the config is invalid, <paramref name="error"/>
    /// carries the reason and the plan is still <c>null</c> — the caller must
    /// then fail the deployment with that error, not fall back to install.sh.
    /// </summary>
    public static DockerDeployPlan? TryParse(string deployJson, out string? error)
    {
        error = null;
        JsonDocument doc;
        try
        {
            doc = JsonDocument.Parse(string.IsNullOrWhiteSpace(deployJson) ? "{}" : deployJson);
        }
        catch (JsonException)
        {
            return null; // install.sh's validator reports malformed configs
        }

        using (doc)
        {
            if (doc.RootElement.ValueKind != JsonValueKind.Object
                || !doc.RootElement.TryGetProperty("endpoints", out var eps)
                || eps.ValueKind != JsonValueKind.Array)
            {
                return null;
            }

            var docker = new List<Endpoint>();
            var other = 0;
            var i = -1;
            foreach (var ep in eps.EnumerateArray())
            {
                i++;
                if (ep.ValueKind != JsonValueKind.Object)
                {
                    other++;
                    continue;
                }
                var provider = ep.TryGetProperty("provider", out var p) && p.ValueKind == JsonValueKind.String
                    ? p.GetString()
                    : null;
                if (!DockerProviderOptions.IsDocker(provider))
                {
                    other++;
                    continue;
                }

                var stacks = new List<string>();
                if (ep.TryGetProperty("http_stacks", out var hs) && hs.ValueKind == JsonValueKind.Array)
                {
                    foreach (var s in hs.EnumerateArray())
                    {
                        if (s.ValueKind == JsonValueKind.String && s.GetString() is { Length: > 0 } name)
                        {
                            stacks.Add(name.Trim().ToLowerInvariant());
                        }
                    }
                }
                if (stacks.Count > 1)
                {
                    error = $"endpoints[{i}]: docker endpoints support exactly one http_stack each (got {string.Join(",", stacks)}); "
                            + "add one endpoint per stack";
                    return null;
                }
                if (ep.TryGetProperty("languages", out var langs) && langs.ValueKind == JsonValueKind.Array && langs.GetArrayLength() > 0)
                {
                    error = $"endpoints[{i}]: reference-API languages are not available on docker targets (the target images ship the endpoint + proxy stacks only)";
                    return null;
                }
                var label = ep.TryGetProperty("label", out var l) && l.ValueKind == JsonValueKind.String && l.GetString() is { Length: > 0 } lb
                    ? lb
                    : $"target-{i + 1}";
                var stack = stacks.Count == 0 ? null : stacks[0];
                if (stack is "none" or "rust")
                {
                    stack = null;
                }
                docker.Add(new Endpoint(i, label, stack));
            }

            if (docker.Count == 0)
            {
                return null;
            }
            if (other > 0)
            {
                error = "a deployment cannot mix docker endpoints with cloud/lan endpoints — create separate deployments";
                return null;
            }
            return new DockerDeployPlan(docker);
        }
    }
}
