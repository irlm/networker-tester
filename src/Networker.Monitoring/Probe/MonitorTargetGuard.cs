using System.Net;
using System.Net.Sockets;

namespace Networker.Monitoring.Probe;

/// <summary>
/// Minimal SSRF guard for the foundation release: rejects targets that are
/// obviously non-global — loopback, link-local (including the cloud metadata
/// endpoint 169.254.169.254), RFC1918 private ranges, the unspecified address,
/// CGNAT, ULA, and multicast/broadcast space. This is address rejection only;
/// DNS-rebinding-grade hardening (pinning the resolved address for the actual
/// connection, redirect re-validation) lands in the hardening PR and is a
/// blocking prerequisite for enabling the scheduler against untrusted target
/// configuration.
/// </summary>
public static class MonitorTargetGuard
{
    public static bool IsBlockedAddress(IPAddress address)
    {
        if (address.IsIPv4MappedToIPv6)
        {
            address = address.MapToIPv4();
        }

        if (IPAddress.IsLoopback(address))
        {
            return true;
        }

        if (address.AddressFamily == AddressFamily.InterNetwork)
        {
            var bytes = address.GetAddressBytes();
            return bytes[0] == 0 // 0.0.0.0/8 ("this network", incl. 0.0.0.0)
                || bytes[0] == 10 // 10.0.0.0/8
                || (bytes[0] == 100 && (bytes[1] & 0xC0) == 64) // 100.64.0.0/10 CGNAT
                || (bytes[0] == 169 && bytes[1] == 254) // 169.254.0.0/16 link-local + metadata
                || (bytes[0] == 172 && (bytes[1] & 0xF0) == 16) // 172.16.0.0/12
                || (bytes[0] == 192 && bytes[1] == 168) // 192.168.0.0/16
                || bytes[0] >= 224; // multicast, reserved, broadcast
        }

        return address.Equals(IPAddress.IPv6Any) // ::
            || address.IsIPv6LinkLocal // fe80::/10
            || address.IsIPv6SiteLocal // fec0::/10 (deprecated but non-global)
            || address.IsIPv6UniqueLocal // fc00::/7
            || address.IsIPv6Multicast; // ff00::/8
    }

    /// <summary>
    /// Create/update-time rejection: literal non-global IPs and hostnames that
    /// can only ever be non-global. Purely syntactic — no DNS resolution here.
    /// </summary>
    public static bool IsObviouslyNonGlobalHost(string host)
    {
        var trimmed = host.Trim().TrimStart('[').TrimEnd(']');
        if (IPAddress.TryParse(trimmed, out var literal))
        {
            return IsBlockedAddress(literal);
        }

        return trimmed.Equals("localhost", StringComparison.OrdinalIgnoreCase)
            || trimmed.EndsWith(".localhost", StringComparison.OrdinalIgnoreCase)
            || trimmed.EndsWith(".local", StringComparison.OrdinalIgnoreCase);
    }

    /// <summary>
    /// Execution-time rejection: resolves the host and returns the first
    /// non-global address, or null when every resolved address is acceptable.
    /// Resolution failures return null — the probe's own connect attempt will
    /// surface them with the proper failure kind.
    /// </summary>
    public static async Task<IPAddress?> FindBlockedAddressAsync(
        string host,
        CancellationToken cancellationToken)
    {
        var trimmed = host.Trim().TrimStart('[').TrimEnd(']');
        if (IPAddress.TryParse(trimmed, out var literal))
        {
            return IsBlockedAddress(literal) ? literal : null;
        }

        if (trimmed.Equals("localhost", StringComparison.OrdinalIgnoreCase)
            || trimmed.EndsWith(".localhost", StringComparison.OrdinalIgnoreCase)
            || trimmed.EndsWith(".local", StringComparison.OrdinalIgnoreCase))
        {
            return IPAddress.Loopback;
        }

        IPAddress[] resolved;
        try
        {
            resolved = await Dns.GetHostAddressesAsync(trimmed, cancellationToken);
        }
        catch (SocketException)
        {
            return null;
        }

        return resolved.FirstOrDefault(IsBlockedAddress);
    }
}
