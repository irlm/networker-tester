namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// Writes a secret-bearing temp file with owner-only (0600) permissions so cloud
/// credentials are never world-readable in the shared temp dir.
///
/// <para>Provisioning shells out to <c>az</c>/<c>aws</c>/<c>gcloud</c> and feeds
/// them secrets through temp files — the SP client secret, the Windows admin
/// password, the GCP service-account key, the <c>deploy.json</c> that carries the
/// minted agent API key, Azure protected-settings, and cloud-init scripts that
/// embed the API key. Written via <see cref="File.WriteAllTextAsync"/> they land
/// at the process umask (0644) in <c>/tmp</c>, readable by every local user for
/// the multi-minute lifetime of the CLI call (quality audit F11). This creates
/// them 0600 atomically instead (mirrors the existing SSH-private-key handling).</para>
///
/// <para><see cref="FileStreamOptions.UnixCreateMode"/> applies the mode at
/// creation (no 0644→0600 race) and is ignored on Windows; the control plane runs
/// on Linux in production. No trailing newline is added, so <c>az</c>'s
/// <c>@file</c> loads the exact bytes as the parameter value.</para>
/// </summary>
internal static class SecretFile
{
    private const UnixFileMode OwnerReadWrite = UnixFileMode.UserRead | UnixFileMode.UserWrite;

    /// <summary>Write <paramref name="content"/> to <paramref name="path"/>,
    /// creating the file 0600.</summary>
    public static async Task WriteAsync(string path, string content, CancellationToken ct = default)
    {
        var options = new FileStreamOptions
        {
            Mode = FileMode.Create,
            Access = FileAccess.Write,
            UnixCreateMode = OwnerReadWrite,
        };
        await using var stream = new FileStream(path, options);
        await stream.WriteAsync(System.Text.Encoding.UTF8.GetBytes(content), ct).ConfigureAwait(false);
    }

    /// <summary>Create a directory 0700 (owner-only) — for the az token-cache dir
    /// (<c>AZURE_CONFIG_DIR</c>) whose access-token JSON is as sensitive as the
    /// credential that minted it.</summary>
    public static void CreateDir0700(string path)
    {
        if (OperatingSystem.IsWindows())
        {
            Directory.CreateDirectory(path);
            return;
        }

        Directory.CreateDirectory(
            path, OwnerReadWrite | UnixFileMode.UserExecute);
    }
}
