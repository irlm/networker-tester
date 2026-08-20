using System.Text;
using Microsoft.Extensions.DependencyInjection;
using Networker.ControlPlane.Endpoints;
using Networker.Security;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// Stages the LagHound <b>sample token</b> for <c>install.sh</c>.
///
/// <para>A deploy config that asks for SDK samples
/// (<c>endpoints[i].sdk_samples</c>) also carries the token those samples will
/// require, as AES-256-GCM ciphertext under the top-level <c>sdk_samples</c>
/// object — the same <see cref="CredentialCipher"/> scheme cloud-account
/// credentials use. Ciphertext, because <c>deployment.config</c> is readable by
/// every project member through <c>GET /deployments</c> and is echoed into the
/// deploy log; the plaintext must exist only inside the control plane and
/// inside the installer's process environment.</para>
///
/// <para>This class is the decrypt step: it hands the runner
/// <c>LAGHOUND_SAMPLE_TOKEN</c> to layer onto install.sh's environment, exactly
/// how <see cref="GcpInstallerCredentials"/> stages the gcloud key. A config
/// with no samples, or a cipher that cannot open the ciphertext (key rotated
/// past what this process holds), yields an EMPTY environment and a note — the
/// installer then mints its own per-host token and the operator re-registers,
/// rather than the deploy dying on a secret it cannot read.</para>
/// </summary>
internal static class SdkSampleInstallerToken
{
    /// <summary>The environment variable install.sh reads the token from.</summary>
    public const string EnvVar = "LAGHOUND_SAMPLE_TOKEN";

    /// <summary>What <see cref="Resolve"/> decided: the env to layer on, and a
    /// human line for the deploy log when something was not as expected.</summary>
    internal sealed record Outcome(IReadOnlyDictionary<string, string> Env, string? Note)
    {
        public static readonly Outcome None =
            new(new Dictionary<string, string>(StringComparer.Ordinal), null);
    }

    /// <summary>
    /// Decrypt the staged sample token out of <paramref name="deployJson"/>.
    /// Never throws: every failure degrades to <see cref="Outcome.None"/> with
    /// a note.
    /// </summary>
    public static Outcome Resolve(IServiceScopeFactory scopeFactory, string deployJson, ILogger logger)
    {
        var cipherText = SdkSampleEndpoints.ReadTokenCipher(deployJson);
        if (cipherText is null)
        {
            return Outcome.None;
        }

        try
        {
            using var scope = scopeFactory.CreateScope();
            var cipher = scope.ServiceProvider.GetService<CredentialCipher>();
            if (cipher is null)
            {
                return new Outcome(Outcome.None.Env,
                    ">> no credential cipher on this control plane — SDK samples will be installed with an installer-minted token");
            }
            var token = Encoding.UTF8.GetString(cipher.Decrypt(cipherText.Value.Enc, cipherText.Value.Nonce));
            if (string.IsNullOrWhiteSpace(token))
            {
                return new Outcome(Outcome.None.Env,
                    ">> staged SDK sample token was empty — the installer will mint one");
            }
            return new Outcome(
                new Dictionary<string, string>(StringComparer.Ordinal) { [EnvVar] = token },
                ">> SDK sample token staged for install.sh");
        }
        catch (Exception ex)
        {
            logger.LogWarning(ex, "Could not decrypt the staged SDK sample token — falling back to an installer-minted one");
            return new Outcome(Outcome.None.Env,
                ">> staged SDK sample token could not be decrypted (key rotation?) — the installer will mint one");
        }
    }

    /// <summary>Merge two installer environments (later wins on a key clash).
    /// Returns null when both are empty so the runner keeps passing null.</summary>
    public static IReadOnlyDictionary<string, string>? Merge(
        IReadOnlyDictionary<string, string>? first, IReadOnlyDictionary<string, string>? second)
    {
        var firstCount = first?.Count ?? 0;
        var secondCount = second?.Count ?? 0;
        if (firstCount == 0 && secondCount == 0)
        {
            return null;
        }
        var merged = new Dictionary<string, string>(StringComparer.Ordinal);
        foreach (var (k, v) in first ?? Outcome.None.Env)
        {
            merged[k] = v;
        }
        foreach (var (k, v) in second ?? Outcome.None.Env)
        {
            merged[k] = v;
        }
        return merged;
    }
}
