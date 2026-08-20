using System.Text.Json;
using System.Text.Json.Nodes;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Normalizes a <c>network</c> endpoint's multi-URL set (#782) before it is
/// stored, so every reader — the run dispatcher, the agent, the probe page's
/// watchlist attribution — sees one canonical shape.
///
/// The endpoint block is polymorphic JSONB stored VERBATIM (see
/// <see cref="TestConfigsEndpoints"/>), which is why this exists: without it a
/// set config could persist a <c>hosts</c> array holding blanks, the same URL
/// twice, or a <c>host</c> that is not <c>hosts[0]</c>, and nothing would
/// notice until the agent expanded it into <c>--target</c> flags on the runner.
/// A duplicated member probes one URL twice while the run's name and per-URL
/// grouping claim two distinct targets; a blank member becomes a guaranteed
/// failed attempt attributed to nobody.
///
/// Deliberately structural only. Scheme / reachability judgements stay on the
/// client, where the user can see and fix them: the tester legitimately probes
/// bare hostnames, IP literals and internal names, and this layer must not
/// start guessing which of those are "real".
/// </summary>
internal static class TestConfigEndpointNormalizer
{
    /// <summary>
    /// Members allowed in one URL set. A set is ONE tester process walking
    /// every target in sequence, so members multiply wall clock against the
    /// run's <c>max_duration_secs</c> watchdog. Mirrors MAX_SET_URLS in
    /// dashboard/src/lib/probe-set.ts — the client caps before sending, and
    /// this is the backstop for API callers that don't.
    /// </summary>
    internal const int MaxSetHosts = 25;

    /// <summary>
    /// Produce the endpoint JSON to store. Returns false with
    /// <paramref name="error"/> set when the set cannot be repaired into
    /// something probeable; the caller turns that into a 400.
    /// <paramref name="normalizedJson"/> is the request's own text whenever
    /// there was nothing to change (including every non-network endpoint and
    /// every classic single-host config, whose bytes are untouched).
    /// </summary>
    internal static bool TryNormalize(JsonElement endpoint, out string normalizedJson, out string? error)
    {
        normalizedJson = endpoint.GetRawText();
        error = null;

        if (endpoint.ValueKind != JsonValueKind.Object) return true;
        if (!endpoint.TryGetProperty("kind", out var kind)
            || kind.ValueKind != JsonValueKind.String
            || kind.GetString() != "network")
        {
            return true;
        }

        // No `hosts` key at all: the classic single-URL shape. Leave it exactly
        // as sent — every pre-#782 config must round-trip byte-identically.
        if (!endpoint.TryGetProperty("hosts", out var hostsProperty)) return true;

        if (hostsProperty.ValueKind != JsonValueKind.Array)
        {
            error = "endpoint.hosts must be an array of URL strings";
            return false;
        }

        var hosts = new List<string>();
        foreach (var element in hostsProperty.EnumerateArray())
        {
            if (element.ValueKind != JsonValueKind.String)
            {
                error = "endpoint.hosts must be an array of URL strings";
                return false;
            }
            var value = element.GetString()?.Trim();
            if (string.IsNullOrEmpty(value)) continue;      // blank member → not a target
            if (hosts.Contains(value, StringComparer.Ordinal)) continue;  // same URL twice → one member
            hosts.Add(value);
        }

        // `host` is the back-compat single-target field and must be hosts[0]:
        // it is what the agent hands apibench and what every pre-set reader
        // uses. A host missing from the list is PREPENDED rather than dropped
        // (the agent does the same) — silently discarding a requested target
        // is the one outcome this must never produce.
        var host = endpoint.TryGetProperty("host", out var hostProperty)
                   && hostProperty.ValueKind == JsonValueKind.String
            ? hostProperty.GetString()?.Trim() ?? string.Empty
            : string.Empty;
        if (host.Length > 0 && !hosts.Contains(host, StringComparer.Ordinal))
        {
            hosts.Insert(0, host);
        }

        if (hosts.Count == 0)
        {
            error = "endpoint.hosts must contain at least one non-empty URL";
            return false;
        }
        if (hosts.Count > MaxSetHosts)
        {
            error = $"endpoint.hosts holds {hosts.Count} URLs; a set is capped at {MaxSetHosts} "
                    + "(one run probes them in sequence — split larger sets across configs)";
            return false;
        }

        // Rewrite only when something actually moved.
        var alreadyCanonical =
            host == hosts[0]
            && hostsProperty.GetArrayLength() == hosts.Count
            && hostsProperty.EnumerateArray()
                .Select((e, i) => e.ValueKind == JsonValueKind.String && e.GetString() == hosts[i])
                .All(same => same);
        if (alreadyCanonical) return true;

        var node = JsonNode.Parse(endpoint.GetRawText())?.AsObject();
        if (node is null) return true;
        node["host"] = hosts[0];
        node["hosts"] = new JsonArray([.. hosts.Select(h => (JsonNode)JsonValue.Create(h))]);
        normalizedJson = node.ToJsonString();
        return true;
    }
}
