using Networker.Security;

namespace Networker.ControlPlane.Security;

/// <summary>
/// Wires the M0 <see cref="CredentialCipher"/> (the byte-compatible port of the
/// Rust dashboard's AES-256-GCM credential encryption) into DI so cloud-account
/// endpoints can encrypt/decrypt secrets. Program.cs calls
/// <see cref="AddCredentialCipher"/> during service registration.
///
/// <para>Key resolution mirrors the Rust <c>config.rs</c> path and the
/// <see cref="Auth.AuthExtensions"/> JWT dev-fallback:</para>
/// <list type="number">
///   <item><c>DASHBOARD_CREDENTIAL_KEY</c> — 64 hex chars (the primary key). This
///     is the exact env var and encoding the Rust dashboard reads, so setting it
///     on both sides makes the ciphers interoperable.</item>
///   <item>If absent/invalid and the host runs as Development
///     (<see cref="Auth.DeploymentEnvironment"/>), a random 32-byte dev key is
///     generated so the app still boots (a warning is logged). The dev key is
///     NOT persisted, so restarts rotate it and previously stored secrets become
///     undecryptable — Development only. Outside Development (including an unset
///     ASPNETCORE_ENVIRONMENT, which is treated as Production) startup FAILS
///     CLOSED with an <see cref="InvalidOperationException"/>. This mirrors how
///     AuthExtensions handles a missing DASHBOARD_JWT_SECRET.</item>
///   <item><c>DASHBOARD_CREDENTIAL_KEY_OLD</c> — optional 64 hex chars, used only
///     as a decrypt-time rotation fallback (matches Rust <c>credential_key_old</c>
///     / <c>decrypt_with_fallback</c>).</item>
/// </list>
///
/// The <see cref="CredentialCipher"/> is registered as a singleton (the key
/// material is fixed for the process lifetime).
/// </summary>
public static class CredentialCipherExtensions
{
    /// <summary>Env var carrying the primary 64-hex-char credential key.</summary>
    public const string KeyEnvVar = "DASHBOARD_CREDENTIAL_KEY";

    /// <summary>Env var carrying the optional 64-hex-char rotation/old key.</summary>
    public const string OldKeyEnvVar = "DASHBOARD_CREDENTIAL_KEY_OLD";

    public static IServiceCollection AddCredentialCipher(this IServiceCollection services)
    {
        byte[] key = ResolvePrimaryKey(services);
        byte[]? oldKey = ResolveOldKey();

        services.AddSingleton(new CredentialCipher(key, oldKey));
        return services;
    }

    private static byte[] ResolvePrimaryKey(IServiceCollection services)
    {
        var hex = Environment.GetEnvironmentVariable(KeyEnvVar);
        if (!string.IsNullOrEmpty(hex))
        {
            try
            {
                // Rust filters on len == 64; KeyFromHex enforces the same length
                // and decodes case-insensitively.
                return CredentialCipher.KeyFromHex(hex);
            }
            catch (Exception ex)
            {
                // FAIL CLOSED outside Development: an invalid key silently
                // replaced by an ephemeral one means every stored cloud secret
                // becomes garbage on the next restart.
                if (!Auth.DeploymentEnvironment.IsDevelopment(services))
                {
                    throw new InvalidOperationException(
                        $"{KeyEnvVar} is set but invalid ({ex.Message}) and the host environment is " +
                        $"{Auth.DeploymentEnvironment.Describe(services)}. Refusing to start with an ephemeral " +
                        "credential key. Provide a 64-hex-char AES-256 key (openssl rand -hex 32).", ex);
                }

                Console.Error.WriteLine(
                    $"networker-controlplane: {KeyEnvVar} is set but invalid ({ex.Message}); " +
                    "generating an insecure in-memory dev key instead (Development only). Stored " +
                    "cloud credentials will be undecryptable until a valid key is provided.");
            }
        }
        else
        {
            // FAIL CLOSED outside Development (an unset/undetectable environment
            // is treated as Production).
            if (!Auth.DeploymentEnvironment.IsDevelopment(services))
            {
                throw new InvalidOperationException(
                    $"{KeyEnvVar} is not set and the host environment is " +
                    $"{Auth.DeploymentEnvironment.Describe(services)}. Refusing to start with an ephemeral " +
                    "credential key: stored cloud-account secrets would become undecryptable on the " +
                    "next restart. Set a stable 64-hex-char AES-256 key (openssl rand -hex 32), or " +
                    "run with ASPNETCORE_ENVIRONMENT=Development for local dev.");
            }

            Console.Error.WriteLine(
                $"networker-controlplane: {KeyEnvVar} not set — generating an insecure " +
                "in-memory dev credential key so the app boots (Development only). This key is " +
                "NOT persisted; set a stable 64-hex-char key (openssl rand -hex 32) in " +
                "production, or cloud-account secrets will become undecryptable across restarts.");
        }

        // Development-only fallback: random 32-byte key so the process starts
        // (mirrors the JWT dev-secret fallback in AuthExtensions). Not written
        // to disk.
        byte[] devKey = new byte[CredentialCipher.KeySize];
        System.Security.Cryptography.RandomNumberGenerator.Fill(devKey);
        return devKey;
    }

    private static byte[]? ResolveOldKey()
    {
        var hex = Environment.GetEnvironmentVariable(OldKeyEnvVar);
        if (string.IsNullOrEmpty(hex))
        {
            return null;
        }

        try
        {
            return CredentialCipher.KeyFromHex(hex);
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine(
                $"networker-controlplane: {OldKeyEnvVar} is set but invalid ({ex.Message}); " +
                "ignoring the rotation fallback key.");
            return null;
        }
    }
}
