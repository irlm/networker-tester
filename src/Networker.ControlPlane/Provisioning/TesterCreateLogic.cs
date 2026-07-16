using System.Security.Cryptography;
using System.Text.Json;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// Pure decision / catalog helpers for <b>tester creation</b> — the C# port of
/// the create-path logic in the Rust dashboard's <c>api/testers.rs</c>
/// (<c>check_rate_limit</c>, body validation, <c>target_triple_for</c>) and
/// <c>services/cloud_provider.rs</c> (<c>resolve_image</c>,
/// <c>default_ssh_user</c>, <c>generate_vm_name</c>,
/// <c>CloudProvider::from_connection</c> config validation) plus the agent
/// api-key minting from <c>generate_agent_api_key</c>.
///
/// <para>Everything here is IO-free (except the two random generators) so the
/// endpoint's gating decisions are unit-testable without a DB — the same split
/// the Rust source uses ("Pure helpers (unit-testable without DB)").</para>
/// </summary>
public static class TesterCreateLogic
{
    /// <summary>Total-tester cap per project (Rust <c>MAX_TESTERS_PER_PROJECT</c>).</summary>
    public const int MaxTestersPerProject = 20;

    /// <summary>Hourly create-burst cap per project (Rust <c>MAX_TESTERS_PER_HOUR</c>).</summary>
    public const int MaxTestersPerHour = 20;

    /// <summary>
    /// Rust <c>check_rate_limit</c>: returns the 429 message when either cap is
    /// violated (total cap checked first), else null. Message text is kept
    /// byte-identical to the Rust source.
    /// </summary>
    public static string? CheckRateLimit(long total, long lastHour)
    {
        if (total >= MaxTestersPerProject)
        {
            return $"project already has {total} testers (max {MaxTestersPerProject})";
        }

        if (lastHour >= MaxTestersPerHour)
        {
            return $"project created {lastHour} testers in the last hour (max {MaxTestersPerHour}/h)";
        }

        return null;
    }

    /// <summary>
    /// The Rust handler's inline body validation: empty (post-trim) name →
    /// "name must not be empty"; empty cloud or region → "cloud and region are
    /// required". Returns the 400 message, or null when valid.
    /// </summary>
    public static string? ValidateCreateBody(string? name, string? cloud, string? region)
    {
        if (string.IsNullOrWhiteSpace(name))
        {
            return "name must not be empty";
        }

        if (string.IsNullOrWhiteSpace(cloud) || string.IsNullOrWhiteSpace(region))
        {
            return "cloud and region are required";
        }

        return null;
    }

    /// <summary>
    /// Resolved column values after the Rust insert's COALESCE defaults
    /// (db/project_testers.rs <c>insert</c>): vm_size 'Standard_B2s', hour 23,
    /// auto_probe FALSE, requested_os 'ubuntu-24.04', requested_variant 'server'.
    /// </summary>
    public sealed record CreateDefaults(
        string VmSize,
        short AutoShutdownLocalHour,
        bool AutoProbeEnabled,
        string RequestedOs,
        string RequestedVariant);

    /// <summary>Apply the COALESCE defaults to the optional body fields.</summary>
    public static CreateDefaults ApplyDefaults(
        string? vmSize, short? autoShutdownLocalHour, bool? autoProbeEnabled,
        string? requestedOs, string? requestedVariant) =>
        new(
            vmSize ?? "Standard_B2s",
            autoShutdownLocalHour ?? 23,
            autoProbeEnabled ?? false,
            requestedOs ?? "ubuntu-24.04",
            requestedVariant ?? "server");

    /// <summary>
    /// Validate a <c>cloud_connection.config</c> JSON payload the way the Rust
    /// <c>CloudProvider::from_connection</c> / per-provider <c>from_config</c>
    /// constructors do. Returns the exact Rust error message on failure, null
    /// when the config would construct a provider successfully.
    /// </summary>
    public static string? ValidateConnectionConfig(string provider, string configJson)
    {
        JsonDocument doc;
        try
        {
            doc = JsonDocument.Parse(string.IsNullOrWhiteSpace(configJson) ? "{}" : configJson);
        }
        catch (JsonException)
        {
            doc = JsonDocument.Parse("{}");
        }

        using (doc)
        {
            var root = doc.RootElement;
            string? Str(string key) =>
                root.ValueKind == JsonValueKind.Object
                && root.TryGetProperty(key, out var v)
                && v.ValueKind == JsonValueKind.String
                    ? v.GetString()
                    : null;

            switch (provider)
            {
                case "azure":
                    if (Str("subscription_id") is null)
                    {
                        return "azure config: missing subscription_id";
                    }

                    if (Str("resource_group") is null)
                    {
                        return "azure config: missing resource_group";
                    }

                    return null;

                case "aws":
                    // AwsProvider::from_config: unwrap_or("") then bail on empty.
                    if (string.IsNullOrEmpty(Str("access_key_id")) || string.IsNullOrEmpty(Str("secret_access_key")))
                    {
                        return "aws config: missing access_key_id or secret_access_key";
                    }

                    return null;

                case "gcp":
                {
                    var jsonKey = Str("json_key");
                    if (jsonKey is null)
                    {
                        return "gcp config: missing json_key";
                    }

                    try
                    {
                        using var keyDoc = JsonDocument.Parse(jsonKey);
                        if (keyDoc.RootElement.ValueKind != JsonValueKind.Object
                            || !keyDoc.RootElement.TryGetProperty("project_id", out var pid)
                            || pid.ValueKind != JsonValueKind.String)
                        {
                            return "gcp json_key: missing project_id";
                        }
                    }
                    catch (JsonException)
                    {
                        return "gcp config: json_key is not valid JSON";
                    }

                    return null;
                }

                default:
                    return $"unsupported cloud provider: {provider}";
            }
        }
    }

    /// <summary>
    /// Rust <c>target_triple_for</c>: map a <c>requested_os</c> string to the
    /// release-asset target triple the bootstrap downloads (x86_64 assumed).
    /// </summary>
    public static string TargetTripleFor(string requestedOs) =>
        requestedOs.StartsWith("windows", StringComparison.Ordinal)
            ? "x86_64-pc-windows-msvc"
            : "x86_64-unknown-linux-musl";

    /// <summary>
    /// Rust <c>cloud_provider::resolve_image</c>: (cloud, os, variant) → the
    /// provider-specific image reference (Azure URN / AWS marker / GCP family).
    /// </summary>
    public static string ResolveImage(string cloud, string os, string variant) => (cloud, os, variant) switch
    {
        // Azure — URN format: Publisher:Offer:Sku:Version.
        // Azure Ubuntu Desktop is not a published image; falls back to server.
        ("azure", "ubuntu-24.04", "server" or "desktop") => "Canonical:ubuntu-24_04-lts:server:latest",
        ("azure", "ubuntu-22.04", "server") => "Canonical:0001-com-ubuntu-server-jammy:22_04-lts-gen2:latest",
        ("azure", "windows-2022", "server") => "MicrosoftWindowsServer:WindowsServer:2022-datacenter-azure-edition:latest",
        ("azure", "windows-11", "desktop") => "MicrosoftWindowsDesktop:windows-11:win11-24h2-pro:latest",
        ("azure", "debian-12", "server") => "Debian:debian-12:12:latest",

        // AWS — marker resolved to an AMI at create time via describe-images.
        ("aws", "ubuntu-24.04", "server") => "aws:ubuntu-24.04-server",
        ("aws", "ubuntu-22.04", "server") => "aws:ubuntu-22.04-server",
        ("aws", "windows-2022", "server") => "aws:windows-2022-server",
        ("aws", "debian-12", "server") => "aws:debian-12-server",

        // GCP — image family.
        ("gcp", "ubuntu-24.04", "server") => "ubuntu-2404-lts-amd64",
        ("gcp", "ubuntu-22.04", "server") => "ubuntu-2204-lts",
        ("gcp", "debian-12", "server") => "debian-12",
        ("gcp", "windows-2022", "server") => "windows-2022",

        // Fallbacks: Ubuntu 24.04 Server.
        ("azure", _, _) => "Canonical:ubuntu-24_04-lts:server:latest",
        ("aws", _, _) => "aws:ubuntu-24.04-server",
        ("gcp", _, _) => "ubuntu-2404-lts-amd64",
        _ => "ubuntu-24.04-server",
    };

    /// <summary>Rust <c>cloud_provider::default_ssh_user</c>.</summary>
    public static string DefaultSshUser(string cloud, string os)
    {
        if (os.StartsWith("windows", StringComparison.Ordinal))
        {
            return "azureadmin"; // "Administrator" is reserved on Azure Windows images
        }

        return (cloud, os) switch
        {
            // Azure disallows "admin" as username — "azureuser" for all Azure Linux.
            ("azure", _) => "azureuser",
            ("aws", "debian-12") => "admin",
            ("gcp", "debian-12") => "admin",
            ("aws", _) => "ubuntu",
            ("gcp", _) => "ubuntu",
            _ => "ubuntu",
        };
    }

    /// <summary>
    /// Rust <c>cloud_provider::generate_vm_name</c>: short, DNS-safe
    /// <c>tester-{region}-{5 hex chars}</c>.
    /// </summary>
    public static string GenerateVmName(string region) =>
        $"tester-{region}-{Guid.NewGuid().ToString("N")[..5]}";

    /// <summary>
    /// Rust <c>generate_agent_api_key</c>: 48-char url-safe alphanumeric random
    /// string. Not secret-level entropy — a per-agent service credential,
    /// rotated on re-provision. Satisfies the bootstrap's ^[A-Za-z0-9]{32,128}$
    /// api-key whitelist.
    /// </summary>
    public static string GenerateAgentApiKey() =>
        new(RandomNumberGenerator.GetItems<char>(
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", 48));
}
