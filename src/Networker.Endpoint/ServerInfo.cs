using System.Runtime.InteropServices;
using System.Text.Json.Serialization;

namespace Networker.Endpoint;

/// <summary>
/// Static server identity constants mirroring the Rust crate.
/// Version matches the workspace <c>Cargo.toml</c> version field.
/// </summary>
public static class ServerInfo
{
    public const string Service = "networker-endpoint";

    /// <summary>Mirrors <c>CARGO_PKG_VERSION</c> (workspace version).</summary>
    public const string Version = "0.28.14";
}

/// <summary>
/// Non-sensitive system metadata exposed via <c>GET /info</c>.
/// Field names + JSON shape mirror the Rust <c>SystemMeta</c> struct exactly.
/// Optional fields are omitted from JSON when null (matching serde
/// <c>skip_serializing_if = "Option::is_none"</c>).
/// </summary>
public sealed class SystemMeta
{
    [JsonPropertyName("os")]
    public string Os { get; init; } = "";

    [JsonPropertyName("arch")]
    public string Arch { get; init; } = "";

    [JsonPropertyName("cpu_cores")]
    public int CpuCores { get; init; }

    [JsonPropertyName("total_memory_mb")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public ulong? TotalMemoryMb { get; init; }

    [JsonPropertyName("os_version")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? OsVersion { get; init; }

    [JsonPropertyName("hostname")]
    public string Hostname { get; init; } = "";

    [JsonPropertyName("region")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? Region { get; init; }

    [JsonPropertyName("public_dns")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? PublicDns { get; init; }

    [JsonPropertyName("public_ip")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? PublicIp { get; init; }

    /// <summary>
    /// Collect system metadata. Cloud-metadata probing (region / public DNS /
    /// public IP) mirrors the Rust <c>detect_*</c> helpers, using the same
    /// Azure/AWS/GCP endpoints and a short timeout so it is a no-op off-cloud.
    /// </summary>
    public static SystemMeta Collect()
    {
        var region = CloudMetadata.DetectRegion();
        return new SystemMeta
        {
            Os = OsName(),
            Arch = ArchName(),
            CpuCores = Environment.ProcessorCount,
            TotalMemoryMb = DetectTotalMemoryMb(),
            OsVersion = DetectOsVersion(),
            Hostname = HostnameResolver.Get(),
            Region = region,
            PublicDns = CloudMetadata.DetectPublicDns(region),
            PublicIp = CloudMetadata.DetectPublicIp(region),
        };
    }

    // Match Rust std::env::consts::OS values.
    private static string OsName()
    {
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Linux)) return "linux";
        if (RuntimeInformation.IsOSPlatform(OSPlatform.OSX)) return "macos";
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows)) return "windows";
        return RuntimeInformation.OSDescription.ToLowerInvariant();
    }

    // Match Rust std::env::consts::ARCH values.
    private static string ArchName() => RuntimeInformation.ProcessArchitecture switch
    {
        Architecture.X64 => "x86_64",
        Architecture.X86 => "x86",
        Architecture.Arm64 => "aarch64",
        Architecture.Arm => "arm",
        var a => a.ToString().ToLowerInvariant(),
    };

    private static ulong? DetectTotalMemoryMb()
    {
        try
        {
            if (RuntimeInformation.IsOSPlatform(OSPlatform.Linux))
            {
                foreach (var line in File.ReadLines("/proc/meminfo"))
                {
                    if (line.StartsWith("MemTotal:", StringComparison.Ordinal))
                    {
                        var parts = line.Substring("MemTotal:".Length)
                            .Split(' ', StringSplitOptions.RemoveEmptyEntries);
                        if (parts.Length > 0 && ulong.TryParse(parts[0], out var kb))
                            return kb / 1024;
                    }
                }
                return null;
            }

            // macOS / Windows: fall back to GC-reported total available memory
            // (best-effort; the Rust crate shells out to sysctl / wmic).
            var info = GC.GetGCMemoryInfo();
            if (info.TotalAvailableMemoryBytes > 0)
                return (ulong)(info.TotalAvailableMemoryBytes / (1024 * 1024));
        }
        catch
        {
            // ignore
        }
        return null;
    }

    private static string? DetectOsVersion()
    {
        try
        {
            if (RuntimeInformation.IsOSPlatform(OSPlatform.Linux))
            {
                if (File.Exists("/etc/os-release"))
                {
                    foreach (var line in File.ReadLines("/etc/os-release"))
                    {
                        if (line.StartsWith("PRETTY_NAME=", StringComparison.Ordinal))
                            return line.Substring("PRETTY_NAME=".Length).Trim('"');
                    }
                }
                return null;
            }
        }
        catch
        {
            // ignore
        }

        var desc = RuntimeInformation.OSDescription.Trim();
        return string.IsNullOrEmpty(desc) ? null : desc;
    }
}
