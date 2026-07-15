using System.Text.Json.Serialization;
using Networker.ControlPlane.Auth;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// C# port of the Rust dashboard's <c>api/bench_tokens.rs</c> — list/revoke the
/// short-lived benchmark API tokens stored in an Azure Key Vault. Mounted in
/// <c>protected_flat</c>; list + single-revoke require any authenticated user
/// (with an ownership gate for non-admins), and bulk revoke-all requires
/// platform admin.
///
/// <para>Routes:</para>
/// <list type="bullet">
///   <item><b>GET /api/bench-tokens</b> — list token metadata (never secrets).
///     Non-admins see only their own tokens.</item>
///   <item><b>DELETE /api/bench-tokens/{name}</b> — revoke one token. Name must
///     start with <c>bench-</c> (else 400). Non-admins must own it (else 403).
///     Returns <c>{ status: "revoked", name }</c>.</item>
///   <item><b>DELETE /api/bench-tokens</b> — revoke ALL bench tokens (admin).
///     Returns <c>{ status: "completed", total, revoked, errors }</c>.</item>
/// </list>
///
/// <para>Token name → (config_id, testbed_id) parsing (<see cref="ParseTokenName"/>)
/// and the per-user filtering are ported faithfully and unit-tested.</para>
///
/// <para><b>Key Vault access (env-gated + stubbed CLI):</b>
/// <c>BENCH_KEYVAULT_NAME</c> selects the vault. When it is unset:
/// list returns the mock token set if <c>BENCH_MOCK_TOKENS=1</c>, else <c>[]</c>
/// (faithful); single-revoke returns 500 (Rust: vault name required); revoke-all
/// returns 500. When a vault IS configured the Rust code shells out to the
/// <c>az keyvault secret</c> CLI. That CLI call is a <c>// TODO(phase3)</c> stub
/// here — CI has no <c>az</c>/vault — surfacing as 500 (matching the Rust
/// "failed to spawn az" branch) rather than fabricating vault data.</para>
/// </summary>
public static class BenchTokensEndpoints
{
    public static IEndpointRouteBuilder MapBenchTokensEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/bench-tokens — list (any authenticated user).
        app.MapGet("/api/bench-tokens", (HttpContext ctx, ILoggerFactory lf) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            var vault = VaultName();
            if (vault is null)
            {
                if (Environment.GetEnvironmentVariable("BENCH_MOCK_TOKENS") == "1")
                {
                    var mock = FilterTokensForUser(MockTokens(), user);
                    return Results.Ok(mock);
                }
                return Results.Ok(Array.Empty<TokenInfo>());
            }

            // TODO(phase3): shell out to `az keyvault secret list` and map the
            // results. Not available in CI → mirror the Rust "spawn failed" 500.
            lf.CreateLogger("Networker.BenchTokens").LogWarning(
                "bench-tokens list: az keyvault CLI access is a phase-3 stub");
            return Results.StatusCode(StatusCodes.Status500InternalServerError);
        }).RequireAuthorization();

        // DELETE /api/bench-tokens/{name} — revoke one token.
        app.MapDelete("/api/bench-tokens/{name}", (
            string name, HttpContext ctx, ILoggerFactory lf) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            // Prevent arbitrary secret deletion — name must start with "bench-".
            if (!name.StartsWith("bench-", StringComparison.Ordinal))
            {
                lf.CreateLogger("Networker.BenchTokens").LogWarning(
                    "Rejected token revocation (name does not start with bench-): {Name} by {Admin}",
                    name, user.Email);
                return Results.BadRequest();
            }

            var vault = VaultName();
            if (vault is null)
            {
                lf.CreateLogger("Networker.BenchTokens").LogError(
                    "BENCH_KEYVAULT_NAME not set, cannot revoke token");
                return Results.StatusCode(StatusCodes.Status500InternalServerError);
            }

            // TODO(phase3): with a vault configured, the Rust code (a) for
            // non-admins runs `az keyvault secret show` to enforce ownership,
            // then (b) `az keyvault secret delete`. Both CLI calls are stubbed
            // here → 500 (matches the Rust spawn-failure branch).
            lf.CreateLogger("Networker.BenchTokens").LogWarning(
                "bench-tokens revoke: az keyvault CLI access is a phase-3 stub ({Name})", name);
            return Results.StatusCode(StatusCodes.Status500InternalServerError);
        }).RequireAuthorization();

        // DELETE /api/bench-tokens — revoke ALL bench tokens (admin only).
        app.MapDelete("/api/bench-tokens", (HttpContext ctx, ILoggerFactory lf) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }
            if (!user.IsPlatformAdmin)
            {
                return Results.StatusCode(StatusCodes.Status403Forbidden);
            }

            var vault = VaultName();
            if (vault is null)
            {
                lf.CreateLogger("Networker.BenchTokens").LogError(
                    "BENCH_KEYVAULT_NAME not set, cannot revoke tokens");
                return Results.StatusCode(StatusCodes.Status500InternalServerError);
            }

            // TODO(phase3): list + delete every `bench-*` secret via `az`. Stubbed
            // → 500 (matches the Rust "list spawn failed" branch).
            lf.CreateLogger("Networker.BenchTokens").LogWarning(
                "bench-tokens revoke-all: az keyvault CLI access is a phase-3 stub");
            return Results.StatusCode(StatusCodes.Status500InternalServerError);
        }).RequireAuthorization();

        return app;
    }

    // ── Pure helpers (unit-tested) ──────────────────────────────────────────

    /// <summary>Key Vault name from env, or null when unset/empty.</summary>
    public static string? VaultName()
    {
        var v = Environment.GetEnvironmentVariable("BENCH_KEYVAULT_NAME");
        return string.IsNullOrEmpty(v) ? null : v;
    }

    /// <summary>
    /// Parse a secret name <c>bench-{config_id}-vm-{testbed_id}</c> into
    /// (config_id, testbed_id). Returns null when the format doesn't match.
    /// </summary>
    public static (string ConfigId, string TestbedId)? ParseTokenName(string name)
    {
        const string prefix = "bench-";
        if (!name.StartsWith(prefix, StringComparison.Ordinal))
        {
            return null;
        }
        var rest = name[prefix.Length..];
        var i = rest.IndexOf("-vm-", StringComparison.Ordinal);
        if (i < 0)
        {
            return null;
        }
        var configId = rest[..i];
        var testbedId = rest[(i + 4)..];
        if (configId.Length == 0 || testbedId.Length == 0)
        {
            return null;
        }
        return (configId, testbedId);
    }

    /// <summary>Admins see all; others see only their own tokens.</summary>
    public static List<TokenInfo> FilterTokensForUser(List<TokenInfo> tokens, AuthUser user)
    {
        if (user.IsPlatformAdmin)
        {
            return tokens;
        }
        var uid = user.UserId.ToString();
        var email = user.Email;
        return tokens
            .Where(t => t.user == uid || t.user == email)
            .ToList();
    }

    private static List<TokenInfo> MockTokens()
    {
        var now = DateTimeOffset.UtcNow;
        string Rfc(DateTimeOffset t) => t.ToString("yyyy-MM-ddTHH:mm:ss.ffffffzzz");
        return new List<TokenInfo>
        {
            new()
            {
                name = "bench-c4da3bda-vm-7b75a519",
                config_id = "c4da3bda",
                testbed_id = "7b75a519",
                created = Rfc(now.AddHours(-2)),
                expires = Rfc(now.AddHours(2)),
                enabled = true,
                user = "admin@localhost",
                project_id = "benchmark-test",
            },
            new()
            {
                name = "bench-a1b2c3d4-vm-eastus-01",
                config_id = "a1b2c3d4",
                testbed_id = "eastus-01",
                created = Rfc(now.AddHours(-5)),
                expires = Rfc(now.AddHours(-1)),
                enabled = false,
                user = "admin@localhost",
                project_id = "benchmark-test",
            },
            new()
            {
                name = "bench-e5f6g7h8-vm-westus-02",
                config_id = "e5f6g7h8",
                testbed_id = "westus-02",
                created = Rfc(now.AddMinutes(-30)),
                expires = Rfc(now.AddHours(3).AddMinutes(30)),
                enabled = true,
                user = "dev@example.com",
                project_id = "perf-testing",
            },
            new()
            {
                name = "bench-i9j0k1l2-vm-eu-west-1",
                config_id = "i9j0k1l2",
                testbed_id = "eu-west-1",
                created = Rfc(now.AddMinutes(-10)),
                expires = Rfc(now.AddMinutes(50)),
                enabled = true,
                user = "admin@localhost",
                project_id = "benchmark-test",
            },
        };
    }

    /// <summary>Token metadata (camelCase wire shape, matching the Rust struct).</summary>
    public sealed class TokenInfo
    {
        [JsonPropertyName("name")] public string name { get; set; } = string.Empty;
        [JsonPropertyName("configId")] public string config_id { get; set; } = string.Empty;
        [JsonPropertyName("testbedId")] public string testbed_id { get; set; } = string.Empty;
        [JsonPropertyName("created")] public string? created { get; set; }
        [JsonPropertyName("expires")] public string? expires { get; set; }
        [JsonPropertyName("enabled")] public bool enabled { get; set; }
        [JsonPropertyName("user")] public string? user { get; set; }
        [JsonPropertyName("projectId")] public string? project_id { get; set; }
    }
}
