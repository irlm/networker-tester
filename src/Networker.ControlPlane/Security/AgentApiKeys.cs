using System.Security.Cryptography;
using System.Text;

namespace Networker.ControlPlane.Security;

/// <summary>
/// Agent api-key hashing (V040): keys are stored as lowercase-hex SHA-256 in
/// <c>agent.api_key_hash</c> and auth compares hashes in constant time —
/// never the plaintext.
///
/// <para><b>Why plain SHA-256, not a password KDF:</b> agent keys are minted
/// by <c>TesterCreateLogic.GenerateAgentApiKey</c> as 48 random alphanumeric
/// chars (~285 bits of entropy) — high-entropy machine credentials, not
/// human passwords, so brute-forcing the digest is infeasible and a slow KDF
/// would only tax the per-connection auth path. This matches how the
/// collab/invite/reset tokens are stored (<c>CollabTokens.Sha256Hex</c>).</para>
///
/// <para><b>Compatibility:</b> the backfill in
/// <c>V040_agent_api_key_hash.sql</c> computes the same digest in SQL
/// (<c>encode(sha256(convert_to(api_key,'UTF8')),'hex')</c>), so fielded
/// agents keep authenticating with their existing plaintext <c>?key=</c> —
/// zero wire-protocol change.</para>
/// </summary>
public static class AgentApiKeys
{
    /// <summary>Lowercase-hex SHA-256 of the plaintext key — the value stored
    /// in and looked up against <c>agent.api_key_hash</c>.</summary>
    public static string HashHex(string apiKey) =>
        Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(apiKey)));

    /// <summary>
    /// Constant-time equality of two hex digests
    /// (<see cref="CryptographicOperations.FixedTimeEquals(ReadOnlySpan{byte}, ReadOnlySpan{byte})"/>;
    /// length mismatch returns <c>false</c>). Defense in depth on top of the
    /// hash-keyed DB lookup: even the digest comparison never short-circuits.
    /// </summary>
    public static bool FixedTimeEqualsHex(string? expectedHex, string? actualHex)
    {
        if (expectedHex is null || actualHex is null)
        {
            return false;
        }

        return CryptographicOperations.FixedTimeEquals(
            Encoding.ASCII.GetBytes(expectedHex),
            Encoding.ASCII.GetBytes(actualHex));
    }
}
