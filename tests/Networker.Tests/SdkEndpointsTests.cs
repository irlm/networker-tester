using System.Net;
using System.Net.Http.Json;
using System.Text;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;

namespace Networker.Tests;

/// <summary>
/// End-to-end tests for the LagHound SDK-endpoint CRUD surface
/// (<c>/api/projects/{id}/sdk-endpoints</c>) against a real Postgres + the
/// booted control plane: RBAC (viewer reads OK, viewer writes 403, operator
/// writes OK), the token is encrypted at rest (never stored plaintext) and
/// write-only (masked on every read), validation via the ApiError envelope,
/// and delete of a missing/foreign id is a flat 404.
/// </summary>
public sealed class SdkEndpointsTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fixture;

    public SdkEndpointsTests(ControlPlaneFixture fixture) => _fixture = fixture;

    private static string Base =>
        $"/api/projects/{ControlPlaneFixture.SeededProjectId}/sdk-endpoints";

    private static object ValidBody(string name, string token = "secret-laghound-token-abc") => new
    {
        name,
        url = "https://customer.example.com",
        token,
        route = "/laghound/echo",
        runs = 10,
    };

    // ── RBAC ─────────────────────────────────────────────────────────────────

    [Fact]
    public async Task Routes_require_authentication()
    {
        var anon = _fixture.CreateClient();
        Assert.Equal(HttpStatusCode.Unauthorized, (await anon.GetAsync(Base)).StatusCode);
        Assert.Equal(HttpStatusCode.Unauthorized,
            (await anon.PostAsJsonAsync(Base, ValidBody("anon"))).StatusCode);
    }

    [Fact]
    public async Task Non_member_project_is_forbidden()
    {
        var resp = await _fixture.CreateAuthenticatedClient()
            .GetAsync("/api/projects/proj-not-a-member/sdk-endpoints");
        Assert.Equal(HttpStatusCode.Forbidden, resp.StatusCode);
    }

    [Fact]
    public async Task Viewer_can_read_but_not_write()
    {
        var viewer = _fixture.CreateViewerClient();

        Assert.Equal(HttpStatusCode.OK, (await viewer.GetAsync(Base)).StatusCode);
        Assert.Equal(HttpStatusCode.Forbidden,
            (await viewer.PostAsJsonAsync(Base, ValidBody("viewer-denied"))).StatusCode);
    }

    // ── Create + read masking ─────────────────────────────────────────────────

    [Fact]
    public async Task Operator_creates_an_endpoint_and_the_token_is_masked_on_read()
    {
        var op = _fixture.CreateAuthenticatedClient();

        var create = await op.PostAsJsonAsync(Base, ValidBody("checkout-api"));
        Assert.Equal(HttpStatusCode.OK, create.StatusCode);
        var created = await create.Content.ReadFromJsonAsync<JsonElement>();
        var id = created.GetProperty("id").GetGuid();

        // The token never comes back — masked, and echoed metadata is present.
        Assert.Equal("sdkprobe", created.GetProperty("mode").GetString());
        Assert.True(created.GetProperty("token_set").GetBoolean());
        Assert.Equal("********", created.GetProperty("token").GetString());
        Assert.Equal("https://customer.example.com/", created.GetProperty("url").GetString());
        Assert.Equal("/laghound/echo", created.GetProperty("route").GetString());

        // GET detail also masks the token.
        var detail = await op.GetFromJsonAsync<JsonElement>($"{Base}/{id}");
        Assert.Equal("********", detail.GetProperty("token").GetString());

        // GET list includes it, still masked.
        var list = await op.GetFromJsonAsync<JsonElement>(Base);
        var row = list.EnumerateArray().Single(e => e.GetProperty("id").GetGuid() == id);
        Assert.Equal("********", row.GetProperty("token").GetString());
    }

    [Fact]
    public async Task Token_is_encrypted_at_rest_never_stored_plaintext()
    {
        var op = _fixture.CreateAuthenticatedClient();
        const string plaintext = "super-secret-token-9f8e7d6c";

        var create = await op.PostAsJsonAsync(Base, ValidBody("enc-at-rest", plaintext));
        Assert.Equal(HttpStatusCode.OK, create.StatusCode);
        var id = (await create.Content.ReadFromJsonAsync<JsonElement>()).GetProperty("id").GetGuid();

        // Read the raw columns straight from Postgres: the stored ciphertext must
        // NOT contain the plaintext bytes, and a nonce must be present.
        await using var db = _fixture.NewDbContext();
        var cfg = await db.TestConfigs.AsNoTracking().SingleAsync(c => c.Id == id);
        Assert.NotNull(cfg.TokenEnc);
        Assert.NotNull(cfg.TokenNonce);
        Assert.Equal(12, cfg.TokenNonce!.Length); // GCM nonce
        Assert.True(cfg.TokenEnc!.Length >= 16);   // >= GCM tag

        var plainBytes = Encoding.UTF8.GetBytes(plaintext);
        Assert.False(ContainsSubsequence(cfg.TokenEnc, plainBytes),
            "stored token_enc must not contain the plaintext token bytes");
    }

    // ── Validation ─────────────────────────────────────────────────────────────

    [Fact]
    public async Task Create_validates_url_and_token_via_the_api_error_envelope()
    {
        var op = _fixture.CreateAuthenticatedClient();

        async Task AssertBad(object body, string fragment)
        {
            var resp = await op.PostAsJsonAsync(Base, body);
            Assert.Equal(HttpStatusCode.BadRequest, resp.StatusCode);
            var env = await resp.Content.ReadFromJsonAsync<JsonElement>();
            Assert.Contains(fragment, env.GetProperty("error").GetString());
        }

        await AssertBad(new { url = "https://x.example.com", token = "t" }, "name");
        await AssertBad(new { name = "no-url", token = "t" }, "url");
        await AssertBad(new { name = "bad-url", url = "not-a-url", token = "t" }, "url");
        await AssertBad(new { name = "no-token", url = "https://x.example.com" }, "token");
        await AssertBad(
            new { name = "bad-route", url = "https://x.example.com", token = "t", route = "no-leading-slash" },
            "route");
    }

    // ── Delete ─────────────────────────────────────────────────────────────────

    [Fact]
    public async Task Operator_deletes_and_a_missing_id_is_404()
    {
        var op = _fixture.CreateAuthenticatedClient();
        var id = (await (await op.PostAsJsonAsync(Base, ValidBody("to-delete")))
            .Content.ReadFromJsonAsync<JsonElement>()).GetProperty("id").GetGuid();

        Assert.Equal(HttpStatusCode.NoContent, (await op.DeleteAsync($"{Base}/{id}")).StatusCode);
        Assert.Equal(HttpStatusCode.NotFound, (await op.DeleteAsync($"{Base}/{id}")).StatusCode);
        Assert.Equal(HttpStatusCode.NotFound, (await op.DeleteAsync($"{Base}/{Guid.NewGuid()}")).StatusCode);
    }

    [Fact]
    public async Task A_non_sdkprobe_config_is_not_visible_through_the_sdk_surface()
    {
        // The seeded generic config (empty workload, no sdkprobe mode) must not
        // appear in the SDK-endpoint list nor be fetchable/deletable via it.
        var op = _fixture.CreateAuthenticatedClient();

        var list = await op.GetFromJsonAsync<JsonElement>(Base);
        Assert.DoesNotContain(
            list.EnumerateArray(),
            e => e.GetProperty("id").GetGuid() == ControlPlaneFixture.SeededConfigId);

        Assert.Equal(HttpStatusCode.NotFound,
            (await op.GetAsync($"{Base}/{ControlPlaneFixture.SeededConfigId}")).StatusCode);
    }

    private static bool ContainsSubsequence(byte[] haystack, byte[] needle)
    {
        if (needle.Length == 0 || needle.Length > haystack.Length)
        {
            return false;
        }
        for (var i = 0; i <= haystack.Length - needle.Length; i++)
        {
            var match = true;
            for (var j = 0; j < needle.Length; j++)
            {
                if (haystack[i + j] != needle[j]) { match = false; break; }
            }
            if (match) { return true; }
        }
        return false;
    }
}
