using System.Diagnostics;
using System.Net.Sockets;
using System.Text;

namespace Networker.Endpoint;

/// <summary>
/// Best-effort hostname resolution mirroring the Rust <c>get_hostname()</c>
/// fallback chain: HOSTNAME env → COMPUTERNAME env → /proc/sys/kernel/hostname
/// → <c>hostname</c> command → "unknown".
/// </summary>
public static class HostnameResolver
{
    public static string Get()
    {
        var h = Environment.GetEnvironmentVariable("HOSTNAME");
        if (!string.IsNullOrEmpty(h)) return h;

        h = Environment.GetEnvironmentVariable("COMPUTERNAME");
        if (!string.IsNullOrEmpty(h)) return h;

        try
        {
            if (File.Exists("/proc/sys/kernel/hostname"))
            {
                var s = File.ReadAllText("/proc/sys/kernel/hostname").Trim();
                if (!string.IsNullOrEmpty(s)) return s;
            }
        }
        catch { /* ignore */ }

        try
        {
            var psi = new ProcessStartInfo("hostname")
            {
                RedirectStandardOutput = true,
                UseShellExecute = false,
            };
            using var p = Process.Start(psi);
            if (p is not null)
            {
                var outp = p.StandardOutput.ReadToEnd().Trim();
                p.WaitForExit(1000);
                if (p.ExitCode == 0 && !string.IsNullOrEmpty(outp)) return outp;
            }
        }
        catch { /* ignore */ }

        return "unknown";
    }
}

/// <summary>
/// Cloud instance-metadata probing mirroring the Rust helpers
/// (<c>detect_cloud_region</c>, <c>detect_public_dns</c>, <c>detect_public_ip</c>).
/// Uses raw HTTP/1.1 GETs to the well-known link-local endpoints with short
/// timeouts so it is a fast no-op when not running on a cloud VM.
/// </summary>
public static class CloudMetadata
{
    /// <summary>
    /// Blocking raw HTTP GET to a metadata endpoint with a short timeout.
    /// Mirrors <c>cloud_metadata_get_raw</c>: 500 ms connect, 1000 ms read,
    /// requires "200" in the status line, returns the trimmed body or null.
    /// </summary>
    private static string? GetRaw(string hostPort, string hostHeader, string pathQuery, (string, string)[] headers)
    {
        try
        {
            var colon = hostPort.LastIndexOf(':');
            var host = hostPort[..colon];
            var port = int.Parse(hostPort[(colon + 1)..]);

            var sb = new StringBuilder();
            sb.Append($"GET {pathQuery} HTTP/1.1\r\nHost: {hostHeader}\r\nConnection: close\r\n");
            foreach (var (k, v) in headers)
                sb.Append($"{k}: {v}\r\n");
            sb.Append("\r\n");

            using var client = new TcpClient();
            if (!client.ConnectAsync(host, port).Wait(500))
                return null;
            client.ReceiveTimeout = 1000;
            using var stream = client.GetStream();
            var req = Encoding.ASCII.GetBytes(sb.ToString());
            stream.Write(req, 0, req.Length);

            var ms = new MemoryStream();
            var buf = new byte[4096];
            try
            {
                int read;
                while ((read = stream.Read(buf, 0, buf.Length)) > 0)
                    ms.Write(buf, 0, read);
            }
            catch { /* read timeout — use what we have */ }

            var resp = Encoding.UTF8.GetString(ms.ToArray());
            var firstLine = resp.Split('\n').FirstOrDefault() ?? "";
            if (!firstLine.Contains("200")) return null;

            var idx = resp.IndexOf("\r\n\r\n", StringComparison.Ordinal);
            if (idx < 0) return null;
            var body = resp[(idx + 4)..].Trim();
            return string.IsNullOrEmpty(body) ? null : body;
        }
        catch
        {
            return null;
        }
    }

    public static string? DetectRegion()
    {
        // Azure
        var r = GetRaw("169.254.169.254:80", "169.254.169.254",
            "/metadata/instance/compute/location?api-version=2021-02-01&format=text",
            new[] { ("Metadata", "true") });
        if (r is not null) return $"azure/{r}";

        // AWS
        r = GetRaw("169.254.169.254:80", "169.254.169.254",
            "/latest/meta-data/placement/region", Array.Empty<(string, string)>());
        if (r is not null) return $"aws/{r}";

        // GCP
        var zone = GetRaw("169.254.169.254:80", "metadata.google.internal",
            "/computeMetadata/v1/instance/zone",
            new[] { ("Metadata-Flavor", "Google") });
        if (zone is not null)
        {
            var z = zone.Split('/').LastOrDefault() ?? zone;
            var lastDash = z.LastIndexOf('-');
            var region = lastDash >= 0 ? z[..lastDash] : z;
            return $"gcp/{region} ({z})";
        }

        return null;
    }

    public static string? DetectPublicDns(string? region)
    {
        var regionStr = region ?? "";

        if (regionStr.StartsWith("azure/", StringComparison.Ordinal))
        {
            var fqdn = GetRaw("169.254.169.254:80", "169.254.169.254",
                "/metadata/instance/compute/fqdnName?api-version=2021-02-01&format=text",
                new[] { ("Metadata", "true") });
            if (!string.IsNullOrEmpty(fqdn)) return fqdn;

            var hostname = HostnameResolver.Get();
            var azureRegion = regionStr.Length > "azure/".Length ? regionStr["azure/".Length..] : "eastus";
            if (string.IsNullOrEmpty(azureRegion)) azureRegion = "eastus";
            return $"{hostname}.{azureRegion}.cloudapp.azure.com";
        }

        if (regionStr.StartsWith("aws/", StringComparison.Ordinal))
        {
            var dns = GetRaw("169.254.169.254:80", "169.254.169.254",
                "/latest/meta-data/public-hostname", Array.Empty<(string, string)>());
            if (!string.IsNullOrEmpty(dns) && !dns.Contains(".internal"))
                return dns;

            var ip = GetRaw("169.254.169.254:80", "169.254.169.254",
                "/latest/meta-data/public-ipv4", Array.Empty<(string, string)>());
            if (ip is not null)
            {
                var awsRegion = regionStr.Length > "aws/".Length ? regionStr["aws/".Length..] : "us-east-1";
                if (string.IsNullOrEmpty(awsRegion)) awsRegion = "us-east-1";
                var ipDashed = ip.Replace('.', '-');
                return awsRegion == "us-east-1"
                    ? $"ec2-{ipDashed}.compute-1.amazonaws.com"
                    : $"ec2-{ipDashed}.{awsRegion}.compute.amazonaws.com";
            }
        }

        if (regionStr.StartsWith("gcp/", StringComparison.Ordinal))
            return HostnameResolver.Get();

        return null;
    }

    public static string? DetectPublicIp(string? region)
    {
        var regionStr = region ?? "";

        if (regionStr.StartsWith("aws/", StringComparison.Ordinal))
            return GetRaw("169.254.169.254:80", "169.254.169.254",
                "/latest/meta-data/public-ipv4", Array.Empty<(string, string)>());

        if (regionStr.StartsWith("azure/", StringComparison.Ordinal))
            return GetRaw("169.254.169.254:80", "169.254.169.254",
                "/metadata/instance/network/interface/0/ipv4/ipAddress/0/publicIpAddress?api-version=2021-02-01&format=text",
                new[] { ("Metadata", "true") });

        if (regionStr.StartsWith("gcp/", StringComparison.Ordinal))
            return GetRaw("169.254.169.254:80", "metadata.google.internal",
                "/computeMetadata/v1/instance/network-interfaces/0/access-configs/0/external-ip",
                new[] { ("Metadata-Flavor", "Google") });

        return null;
    }
}
