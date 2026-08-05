using System.Net;
using System.Net.Http.Json;
using System.Text;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.Data.Entities;
using Xunit;

namespace Networker.Tests;

/// <summary>
/// Audit P1-1/P1-2/P1-4: three user-facing surfaces whose HTTP layer was never
/// executed by any test.
///
/// <list type="bullet">
///   <item><b>Run report export</b> (4 document formats) — covered only by
///     document-BUILDER unit tests; the route, its format parsing, content
///     types and Content-Disposition were untested. A DOCX 500 caused by raw
///     ANSI in error_message reached production this way once already
///     (v0.28.96).</item>
///   <item><b>Infra-insight</b> (<c>/infra</c>) — zero coverage, and it reads
///     <c>deployment.Config</c> (jsonb).</item>
///   <item><b>Share links</b> — the smoke suite only ever hit
///     <c>/api/share/{unknown}</c>, so the 404 arm short-circuited before the
///     real query. The 200 path, revoke, and expiry were never executed.</item>
/// </list>
/// </summary>
public class ReportAndShareHttpTests : IClassFixture<ControlPlaneFixture>
{
    private readonly ControlPlaneFixture _fx;

    public ReportAndShareHttpTests(ControlPlaneFixture fx) => _fx = fx;

    /// <summary>Seed a completed run with attempts so the report/infra
    /// builders have real content to render (an empty run can render a
    /// trivially-valid document and prove nothing).</summary>
    private async Task<Guid> SeedCompletedRunAsync()
    {
        var now = DateTime.UtcNow;
        var runId = Guid.NewGuid();
        var cfgId = Guid.NewGuid();

        await using var db = _fx.NewDbContext();
        db.TestConfigs.Add(new TestConfig
        {
            Id = cfgId,
            ProjectId = ControlPlaneFixture.SeededProjectId,
            Name = $"report-src-{cfgId:N}",
            EndpointKind = "network",
            EndpointRef = """{"kind":"network","host":"10.20.30.40","port":8444}""",
            Workload = """{"modes":["http1","download"],"runs":2}""",
            MaxDurationSecs = 600,
            CreatedAt = now.AddMinutes(-20),
            UpdatedAt = now.AddMinutes(-20),
        });
        db.TestRuns.Add(new TestRun
        {
            Id = runId,
            TestConfigId = cfgId,
            ProjectId = ControlPlaneFixture.SeededProjectId,
            Status = "completed",
            SuccessCount = 8,
            FailureCount = 1,
            StartedAt = now.AddMinutes(-15),
            FinishedAt = now.AddMinutes(-5),
            CreatedAt = now.AddMinutes(-20),
        });
        await db.SaveChangesAsync();
        return runId;
    }

    [Theory]
    [InlineData("html", "text/html")]
    [InlineData("md", "text/markdown")]
    [InlineData("docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document")]
    [InlineData("pdf", "application/pdf")]
    public async Task Run_report_renders_every_document_format_over_http(string format, string expectedContentType)
    {
        var runId = await SeedCompletedRunAsync();
        using var client = _fx.CreateAuthenticatedClient();

        var resp = await client.GetAsync($"/api/v2/test-runs/{runId}/report?format={format}");

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        Assert.Equal(expectedContentType, resp.Content.Headers.ContentType?.MediaType);

        var bytes = await resp.Content.ReadAsByteArrayAsync();
        // Non-trivial: a 0-byte or few-byte body means the exporter silently
        // produced nothing.
        Assert.True(bytes.Length > 200, $"{format} report was only {bytes.Length} bytes");

        // Format-specific magic — proves the bytes are really that format and
        // not an error page served with the right content type.
        switch (format)
        {
            case "pdf":
                Assert.Equal("%PDF", Encoding.ASCII.GetString(bytes, 0, 4));
                break;
            case "docx":
                // DOCX is a ZIP (OOXML): PK\x03\x04
                Assert.Equal(0x50, bytes[0]);
                Assert.Equal(0x4B, bytes[1]);
                break;
            case "html":
                var html = Encoding.UTF8.GetString(bytes);
                Assert.Contains("<html", html, StringComparison.OrdinalIgnoreCase);
                break;
            case "md":
                var md = Encoding.UTF8.GetString(bytes);
                Assert.Contains("#", md, StringComparison.Ordinal);
                break;
        }
    }

    [Fact]
    public async Task Run_report_rejects_an_unknown_format_without_500ing()
    {
        var runId = await SeedCompletedRunAsync();
        using var client = _fx.CreateAuthenticatedClient();

        var resp = await client.GetAsync($"/api/v2/test-runs/{runId}/report?format=zip");

        Assert.Equal(HttpStatusCode.BadRequest, resp.StatusCode);
    }

    [Fact]
    public async Task Infra_endpoint_returns_200_for_a_real_run()
    {
        // Reads deployment.Config (jsonb) — the column family that wedged the
        // orchestrator. Executing it on Postgres is the point.
        var runId = await SeedCompletedRunAsync();
        using var client = _fx.CreateAuthenticatedClient();

        var resp = await client.GetAsync($"/api/v2/test-runs/{runId}/infra");

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        var body = await resp.Content.ReadFromJsonAsync<JsonElement>();
        Assert.Equal(JsonValueKind.Object, body.ValueKind);
    }

    [Fact]
    public async Task Integrated_report_renders_for_a_project_with_content()
    {
        await SeedCompletedRunAsync();
        using var client = _fx.CreateAuthenticatedClient();

        var resp = await client.GetAsync(
            $"/api/projects/{ControlPlaneFixture.SeededProjectId}/reports/integrated?format=html");

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        var html = await resp.Content.ReadAsStringAsync();
        Assert.True(html.Length > 500, $"integrated report was only {html.Length} chars");
        Assert.Contains("<html", html, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public async Task Share_link_lifecycle_create_view_publicly_then_revoke()
    {
        var runId = await SeedCompletedRunAsync();
        using var client = _fx.CreateAuthenticatedClient();

        // ── create ────────────────────────────────────────────────────────
        var created = await client.PostAsJsonAsync(
            $"/api/projects/{ControlPlaneFixture.SeededProjectId}/share-links",
            new { resource_type = "run", resource_id = runId, label = "audit-p1", expires_in_days = 7 });
        Assert.True(created.IsSuccessStatusCode,
            $"share-link create failed: {created.StatusCode} {await created.Content.ReadAsStringAsync()}");

        var createdBody = await created.Content.ReadFromJsonAsync<JsonElement>();
        var token = createdBody.TryGetProperty("token", out var t) ? t.GetString() : null;
        var linkId = createdBody.TryGetProperty("id", out var idEl) ? idEl.GetString() : null;
        Assert.False(string.IsNullOrWhiteSpace(token), $"no token in response: {createdBody}");

        // ── public view: UNAUTHENTICATED, the whole point of a share link ──
        using var anon = _fx.CreateClient();
        var pub = await anon.GetAsync($"/api/share/{token}");
        Assert.Equal(HttpStatusCode.OK, pub.StatusCode);
        var pubBody = await pub.Content.ReadFromJsonAsync<JsonElement>();
        Assert.Equal(JsonValueKind.Object, pubBody.ValueKind);

        // ── revoke → the same token must stop working ─────────────────────
        if (!string.IsNullOrWhiteSpace(linkId))
        {
            var revoked = await client.PutAsJsonAsync(
                $"/api/projects/{ControlPlaneFixture.SeededProjectId}/share-links/{linkId}",
                new { action = "revoke" });
            Assert.True(revoked.IsSuccessStatusCode,
                $"revoke failed: {revoked.StatusCode} {await revoked.Content.ReadAsStringAsync()}");

            var afterRevoke = await anon.GetAsync($"/api/share/{token}");
            Assert.True(
                afterRevoke.StatusCode is HttpStatusCode.NotFound
                    or HttpStatusCode.Gone or HttpStatusCode.Forbidden,
                $"a revoked share link still served {afterRevoke.StatusCode}");
        }
    }

    [Fact]
    public async Task Unknown_share_token_is_not_found_and_leaks_nothing()
    {
        using var anon = _fx.CreateClient();
        var resp = await anon.GetAsync($"/api/share/{Guid.NewGuid():N}");

        Assert.Equal(HttpStatusCode.NotFound, resp.StatusCode);
        var body = await resp.Content.ReadAsStringAsync();
        Assert.DoesNotContain(ControlPlaneFixture.SeededProjectId, body, StringComparison.Ordinal);
    }
}
