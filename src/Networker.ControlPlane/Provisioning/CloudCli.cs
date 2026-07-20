namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// Cloud-CLI + home-directory resolution for the provisioning shell-outs
/// (fidelity audit F3/F12).
///
/// <para><b>Binary resolution:</b> every cloud CLI honours an env-var override
/// — <c>AZ_CMD</c> (the Rust-era <c>az_bin()</c> shim), and, symmetrically,
/// <c>AWS_CMD</c> and <c>GCLOUD_CMD</c>. Overrides matter beyond dev shims:
/// under the systemd unit a snap-installed <c>gcloud</c> lives in
/// <c>/snap/bin</c>, which is not on systemd's default PATH. When a CLI fails
/// to launch, <see cref="LaunchFailureMessage"/> names the binary AND the
/// override var so the soft-fail is diagnosable instead of silent.</para>
///
/// <para><b>Home resolution:</b> systemd system units do not reliably export
/// <c>$HOME</c>. The old <c>GetEnvironmentVariable("HOME") ?? ""</c> pattern
/// degraded to the *relative* path <c>.ssh/id_rsa.pub</c> under cwd
/// <c>/</c> — silently skipping AWS key-pair import, GCP ssh-keys metadata,
/// and emitting a false <c>gcp_no_local_ssh_key</c> precheck warning.
/// <see cref="HomeDirectory()"/> falls back to the passwd-backed user profile
/// (<see cref="Environment.SpecialFolder.UserProfile"/>) and finally
/// <c>/root</c>.</para>
/// </summary>
public static class CloudCli
{
    /// <summary>Env var overriding the <c>az</c> binary path (Rust parity).</summary>
    public const string AzOverrideVar = "AZ_CMD";

    /// <summary>Env var overriding the <c>aws</c> binary path.</summary>
    public const string AwsOverrideVar = "AWS_CMD";

    /// <summary>Env var overriding the <c>gcloud</c> binary path.</summary>
    public const string GcloudOverrideVar = "GCLOUD_CMD";

    /// <summary>Resolve the Azure CLI binary (<c>AZ_CMD</c> override, else <c>az</c>).</summary>
    public static string AzBin() => Resolve("az", AzOverrideVar, Environment.GetEnvironmentVariable);

    /// <summary>Resolve the AWS CLI binary (<c>AWS_CMD</c> override, else <c>aws</c>).</summary>
    public static string AwsBin() => Resolve("aws", AwsOverrideVar, Environment.GetEnvironmentVariable);

    /// <summary>Resolve the gcloud CLI binary (<c>GCLOUD_CMD</c> override, else <c>gcloud</c>).</summary>
    public static string GcloudBin() => Resolve("gcloud", GcloudOverrideVar, Environment.GetEnvironmentVariable);

    /// <summary>Testable core of the Bin() resolvers.</summary>
    internal static string Resolve(string defaultName, string overrideVar, Func<string, string?> getEnv) =>
        getEnv(overrideVar) is { Length: > 0 } o ? o : defaultName;

    /// <summary>
    /// The override env var for a (possibly already overridden) CLI file name,
    /// or null for a binary this class doesn't own. Matches on the file's base
    /// name so absolute override paths (<c>/snap/bin/gcloud</c>) still map.
    /// </summary>
    public static string? OverrideVarFor(string file) =>
        Path.GetFileNameWithoutExtension(file) switch
        {
            "az" => AzOverrideVar,
            "aws" => AwsOverrideVar,
            "gcloud" => GcloudOverrideVar,
            _ => null,
        };

    /// <summary>
    /// Human-actionable message for a CLI that failed to launch: names the
    /// binary, the failure, and the env var that overrides its path — audit
    /// F12's "no silent soft-fail" contract.
    /// </summary>
    public static string LaunchFailureMessage(string file, string reason)
    {
        var hint = OverrideVarFor(file) is { } overrideVar
            ? $" Install it on the control-plane host and ensure it is on the service's PATH " +
              $"(snap installs live in /snap/bin — see deploy/alethedash-cs.service), " +
              $"or set {overrideVar} to its absolute path."
            : string.Empty;
        return $"failed to launch '{file}': {reason}.{hint}";
    }

    /// <summary>
    /// The service user's home directory: <c>$HOME</c> when set, else the
    /// passwd-backed <see cref="Environment.SpecialFolder.UserProfile"/>, else
    /// <c>/root</c> (non-Windows). Never returns empty on Unix, so callers
    /// building <c>~/.ssh</c> paths can't silently degrade to a relative path.
    /// </summary>
    public static string HomeDirectory() =>
        HomeDirectory(
            Environment.GetEnvironmentVariable,
            () => Environment.GetFolderPath(Environment.SpecialFolder.UserProfile));

    /// <summary>Testable core of <see cref="HomeDirectory()"/>.</summary>
    internal static string HomeDirectory(Func<string, string?> getEnv, Func<string?> getUserProfile)
    {
        if (getEnv("HOME") is { } home && !string.IsNullOrWhiteSpace(home))
        {
            return home;
        }

        if (getUserProfile() is { } profile && !string.IsNullOrWhiteSpace(profile))
        {
            return profile;
        }

        // Last resort for a systemd unit with no User= and a stripped
        // environment: root's home, not "" (which yields relative paths).
        return OperatingSystem.IsWindows() ? string.Empty : "/root";
    }
}
