using System.Net;
using System.Text.Json;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// How a deployed endpoint is addressed when a run targets it.
///
/// <para>A deployment row carries two parallel JSON arrays (V003 + V050):
/// <c>endpoint_ips[i]</c> — what the deploy captured as endpoint i's address
/// (public IP, or the cloud FQDN when install.sh printed one; the legacy
/// column every card / probe / teardown lookup keys off) — and
/// <c>endpoint_hosts[i]</c> — endpoint i's resolvable DNS name when the
/// provider gave one (Azure <c>&lt;label&gt;.&lt;region&gt;.cloudapp.azure.com</c>,
/// AWS <c>ec2-….compute.amazonaws.com</c>, the docker provider's container
/// name, the lab's <c>target-N.lab</c>), else null.</para>
///
/// <para><see cref="PreferredHost"/> is what dispatch hands the tester: the
/// hostname when one was recorded, the IP otherwise. Connecting by hostname
/// is what makes HTTP/3 through IIS possible — http.sys completes the QUIC
/// handshake only for TLS SNI, an IP literal carries none (RFC 6066), and the
/// installer binds IIS's :8445 SNI listener + certificate SAN to that same
/// name (<c>install.ps1 -Setup iis -Fqdn</c> / <c>_iis_setup_powershell</c>).
/// The Linux stacks answer either way (self-signed certs, <c>insecure</c> is
/// injected on proxy dispatch). Pre-V050 rows have no hostnames and resolve
/// exactly as before.</para>
/// </summary>
public static class EndpointAddressing
{
    /// <summary>Parse a JSON string array (the two jsonb columns) into trimmed
    /// entries; non-string / blank elements become null so indexes stay
    /// aligned. Null / malformed → empty.</summary>
    public static IReadOnlyList<string?> ParseArray(string? json)
    {
        if (string.IsNullOrWhiteSpace(json))
        {
            return [];
        }
        try
        {
            using var doc = JsonDocument.Parse(json);
            if (doc.RootElement.ValueKind != JsonValueKind.Array)
            {
                return [];
            }
            var list = new List<string?>();
            foreach (var el in doc.RootElement.EnumerateArray())
            {
                var s = el.ValueKind == JsonValueKind.String ? el.GetString()?.Trim() : null;
                list.Add(string.IsNullOrEmpty(s) ? null : s);
            }
            return list;
        }
        catch (JsonException)
        {
            return [];
        }
    }

    /// <summary>
    /// The host the tester should connect to for the deployment's first
    /// endpoint: <c>endpoint_hosts[i]</c> when recorded for the first
    /// addressed endpoint <c>i</c>, else <c>endpoint_ips[i]</c>. Null when the
    /// deployment captured nothing (a permanent condition — the caller fails
    /// the run instead of spinning).
    /// </summary>
    public static string? PreferredHost(string? endpointIps, string? endpointHosts)
    {
        var ips = ParseArray(endpointIps);
        var hosts = ParseArray(endpointHosts);
        for (var i = 0; i < ips.Count; i++)
        {
            if (ips[i] is null)
            {
                continue;
            }
            return i < hosts.Count && hosts[i] is { } h ? h : ips[i];
        }
        // No ip captured at all: a recorded hostname alone is still addressable.
        return hosts.FirstOrDefault(h => h is not null);
    }

    /// <summary>Every name the deployment's endpoints are known by — ips first,
    /// then hostnames — distinct, non-empty. Used where a run's endpoint_ref is
    /// matched back to a deployment (teardown deferral), since the ref carries
    /// whichever form dispatch chose.</summary>
    public static IReadOnlyList<string> AllHosts(string? endpointIps, string? endpointHosts)
    {
        var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var list = new List<string>();
        foreach (var s in ParseArray(endpointIps).Concat(ParseArray(endpointHosts)))
        {
            if (s is not null && seen.Add(s))
            {
                list.Add(s);
            }
        }
        return list;
    }

    /// <summary>True for an IPv4/IPv6 literal (no SNI on the wire).</summary>
    public static bool IsIpLiteral(string? host) =>
        !string.IsNullOrWhiteSpace(host) && IPAddress.TryParse(host.Trim(), out _);
}
