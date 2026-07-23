using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// <see cref="SecretFile"/> is the whole point of quality-audit F11: cloud
/// secrets fed to az/aws/gcloud through temp files must be owner-only (0600),
/// not the umask-default 0644 that left them world-readable in <c>/tmp</c>.
/// These assert the actual Unix mode on Linux (where the control plane runs and
/// where the C# suite executes in CI); they no-op on Windows, where Unix modes
/// don't apply.
/// </summary>
public sealed class SecretFileTests
{
    [Fact]
    public async Task WriteAsync_creates_the_file_owner_read_write_only()
    {
        var path = Path.Combine(Path.GetTempPath(), $"secretfile-test-{Guid.NewGuid():N}");
        try
        {
            await SecretFile.WriteAsync(path, "s3cr3t-value", default);

            Assert.Equal("s3cr3t-value", await File.ReadAllTextAsync(path));

            if (!OperatingSystem.IsWindows())
            {
                var mode = File.GetUnixFileMode(path);
                Assert.Equal(UnixFileMode.UserRead | UnixFileMode.UserWrite, mode);
                // Explicitly: no group/other bits — the whole vulnerability.
                Assert.False(mode.HasFlag(UnixFileMode.GroupRead));
                Assert.False(mode.HasFlag(UnixFileMode.OtherRead));
            }
        }
        finally
        {
            File.Delete(path);
        }
    }

    [Fact]
    public async Task WriteAsync_writes_exact_bytes_with_no_trailing_newline()
    {
        // az's @file loads the exact file content as the parameter value — a
        // stray newline would corrupt a password/secret.
        var path = Path.Combine(Path.GetTempPath(), $"secretfile-nonl-{Guid.NewGuid():N}");
        try
        {
            await SecretFile.WriteAsync(path, "no-newline", default);
            Assert.Equal("no-newline", await File.ReadAllTextAsync(path));
            Assert.Equal(10, new FileInfo(path).Length); // exactly "no-newline"
        }
        finally
        {
            File.Delete(path);
        }
    }

    [Fact]
    public void CreateDir0700_creates_an_owner_only_directory()
    {
        var path = Path.Combine(Path.GetTempPath(), $"secretdir-test-{Guid.NewGuid():N}");
        try
        {
            SecretFile.CreateDir0700(path);
            Assert.True(Directory.Exists(path));

            if (!OperatingSystem.IsWindows())
            {
                var mode = File.GetUnixFileMode(path);
                Assert.Equal(
                    UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute,
                    mode);
            }
        }
        finally
        {
            Directory.Delete(path);
        }
    }
}
