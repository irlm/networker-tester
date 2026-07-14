using System.IdentityModel.Tokens.Jwt;
using System.Text.Json;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Tests for the per-command JWT minting — must reproduce the Rust
/// <c>auth::commands::mint_command_token</c> claim shape
/// (<c>{sub, aud, scope, exp, iat}</c>) and the <c>agent_dispatch</c> lifetime
/// rule (<c>max(timeout + 60, 300)</c>) so tokens validate against the Rust
/// agent's <c>validate_command_token</c> unchanged.
/// </summary>
public class CommandTokenTests
{
    private const string Secret = "unit-test-secret-at-least-32-bytes-long!";

    private static JwtSecurityToken Decode(string token)
        => new JwtSecurityTokenHandler().ReadJwtToken(token);

    // ── Lifetime rule (Rust token_lifetime_respects_floor) ───────────────────

    [Fact]
    public void Short_timeouts_hit_the_300s_floor()
    {
        Assert.Equal(300, AgentCommandsEndpoints.CommandTokenLifetimeSecs(5));
        Assert.Equal(300, AgentCommandsEndpoints.CommandTokenLifetimeSecs(0));
        Assert.Equal(300, AgentCommandsEndpoints.CommandTokenLifetimeSecs(240));
    }

    [Fact]
    public void Long_timeouts_extend_past_the_floor_with_60s_buffer()
    {
        Assert.Equal(660, AgentCommandsEndpoints.CommandTokenLifetimeSecs(600));
        Assert.Equal(300, AgentCommandsEndpoints.CommandTokenLifetimeSecs(239));
        Assert.Equal(301, AgentCommandsEndpoints.CommandTokenLifetimeSecs(241));
    }

    [Fact]
    public void Default_timeout_is_60s_like_rust_dispatch_body()
    {
        Assert.Equal(60, AgentCommandsEndpoints.DefaultTimeoutSecs);
        // 60s default timeout still floors at the 300s minimum lifetime.
        Assert.Equal(300, AgentCommandsEndpoints.CommandTokenLifetimeSecs(
            AgentCommandsEndpoints.DefaultTimeoutSecs));
    }

    // ── Claim shape ──────────────────────────────────────────────────────────

    [Fact]
    public void Token_carries_rust_command_claims()
    {
        var tokens = new JwtTokenService(Secret);
        var agentId = Guid.NewGuid();
        var configId = Guid.NewGuid();

        var jwt = AgentCommandsEndpoints.MintCommandToken(tokens, agentId, configId, "health", 600);
        var decoded = Decode(jwt);

        Assert.Equal("HS256", decoded.Header.Alg);
        Assert.Equal(agentId.ToString(), decoded.Payload["sub"]);
        Assert.Equal(configId.ToString(), decoded.Payload["aud"]?.ToString());

        var iat = Convert.ToInt64(decoded.Payload["iat"]);
        var exp = Convert.ToInt64(decoded.Payload["exp"]);
        Assert.Equal(600, exp - iat);
    }

    [Fact]
    public void Adhoc_token_uses_empty_string_audience()
    {
        // Rust: config_id None → aud = "" (NOT an absent claim).
        var tokens = new JwtTokenService(Secret);
        var jwt = AgentCommandsEndpoints.MintCommandToken(tokens, Guid.NewGuid(), null, "health", 300);
        var decoded = Decode(jwt);

        Assert.True(decoded.Payload.ContainsKey("aud"));
        Assert.Equal(string.Empty, decoded.Payload["aud"]?.ToString());
    }

    [Fact]
    public void Scope_is_a_json_array_containing_the_verb()
    {
        var tokens = new JwtTokenService(Secret);
        var jwt = AgentCommandsEndpoints.MintCommandToken(
            tokens, Guid.NewGuid(), null, "start_server", 300);

        // Decode the raw payload segment so we see the actual JSON types.
        var payloadSegment = jwt.Split('.')[1];
        var padded = payloadSegment.PadRight(payloadSegment.Length + (4 - payloadSegment.Length % 4) % 4, '=');
        var json = System.Text.Encoding.UTF8.GetString(
            Convert.FromBase64String(padded.Replace('-', '+').Replace('_', '/')));

        using var doc = JsonDocument.Parse(json);
        var scope = doc.RootElement.GetProperty("scope");
        Assert.Equal(JsonValueKind.Array, scope.ValueKind);
        Assert.Single(scope.EnumerateArray());
        Assert.Equal("start_server", scope[0].GetString());

        // exp/iat must be JSON numbers (serde u64), not strings.
        Assert.Equal(JsonValueKind.Number, doc.RootElement.GetProperty("exp").ValueKind);
        Assert.Equal(JsonValueKind.Number, doc.RootElement.GetProperty("iat").ValueKind);
    }

    [Fact]
    public void Tokens_signed_with_different_secrets_differ()
    {
        var agentId = Guid.NewGuid();
        var a = AgentCommandsEndpoints.MintCommandToken(
            new JwtTokenService("secret-a-secret-a-secret-a-secret-a"), agentId, null, "health", 300);
        var b = AgentCommandsEndpoints.MintCommandToken(
            new JwtTokenService("secret-b-secret-b-secret-b-secret-b"), agentId, null, "health", 300);

        // Same claims, different signature segment.
        Assert.NotEqual(a.Split('.')[2], b.Split('.')[2]);
    }
}
