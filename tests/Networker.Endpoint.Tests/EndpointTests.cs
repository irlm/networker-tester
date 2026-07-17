using System.Net;
using System.Net.Http.Json;
using System.Text;
using System.Text.Json;
using Microsoft.AspNetCore.Mvc.Testing;

namespace Networker.Endpoint.Tests;

/// <summary>
/// WebApplicationFactory-based integration tests validating that the ported
/// C# endpoint reproduces the Rust server's route contracts.
/// </summary>
public class EndpointTests : IClassFixture<WebApplicationFactory<Program>>
{
    private readonly WebApplicationFactory<Program> _factory;

    public EndpointTests(WebApplicationFactory<Program> factory) => _factory = factory;

    [Fact]
    public async Task Health_Returns_Ok_Json()
    {
        var client = _factory.CreateClient();
        var resp = await client.GetAsync("/health");

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        var json = await resp.Content.ReadFromJsonAsync<JsonElement>();
        Assert.Equal("ok", json.GetProperty("status").GetString());
        Assert.Equal("networker-endpoint", json.GetProperty("service").GetString());
        // Orchestrator contract (API-SPEC.md §5.1): runtime + version required.
        Assert.Equal("dotnet", json.GetProperty("runtime").GetString());
        Assert.False(string.IsNullOrEmpty(json.GetProperty("version").GetString()));
        Assert.True(resp.Headers.Contains("x-networker-server-timestamp"));
        Assert.True(resp.Headers.Contains("x-networker-server-version"));
    }

    [Fact]
    public async Task Health_Body_Is_Constant()
    {
        var client = _factory.CreateClient();
        var b1 = await client.GetStringAsync("/health");
        var b2 = await client.GetStringAsync("/health");
        Assert.Equal(b1, b2); // constant-work /health (API-SPEC.md §5.1)
    }

    [Fact]
    public async Task Download_Returns_Exactly_N_Bytes_OctetStream()
    {
        var client = _factory.CreateClient();
        var resp = await client.GetAsync("/download?bytes=256");

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        Assert.Equal("application/octet-stream", resp.Content.Headers.ContentType?.MediaType);
        var body = await resp.Content.ReadAsByteArrayAsync();
        Assert.Equal(256, body.Length);
        Assert.All(body, b => Assert.Equal(0x42, b)); // fill byte per API-SPEC.md §5.2
        Assert.True(resp.Content.Headers.Contains("x-download-bytes")
                    || resp.Headers.Contains("x-download-bytes"));
    }

    [Fact]
    public async Task Download_Path_Form_Returns_Exact_Fill_Bytes()
    {
        // Canonical orchestrator form: GET /download/{size} (API-SPEC.md §5.2).
        var client = _factory.CreateClient();
        var resp = await client.GetAsync("/download/1024");

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        var body = await resp.Content.ReadAsByteArrayAsync();
        Assert.Equal(1024, body.Length);
        Assert.All(body, b => Assert.Equal(0x42, b));
    }

    [Fact]
    public async Task Download_Path_Form_Rejects_Non_Integer()
    {
        var client = _factory.CreateClient();
        var resp = await client.GetAsync("/download/abc");
        Assert.Equal(HttpStatusCode.BadRequest, resp.StatusCode);
    }

    [Fact]
    public async Task Download_Default_Is_1024_Bytes()
    {
        var client = _factory.CreateClient();
        var body = await client.GetByteArrayAsync("/download");
        Assert.Equal(1024, body.Length);
    }

    [Fact]
    public async Task Download_Has_ServerTiming_Header()
    {
        var client = _factory.CreateClient();
        var resp = await client.GetAsync("/download?bytes=64");
        var st = ServerTiming(resp);
        Assert.Contains("proc;dur=", st);
    }

    [Fact]
    public async Task Upload_Echoes_Size_And_RequestId()
    {
        var client = _factory.CreateClient();
        var payload = Encoding.UTF8.GetBytes("hello world 12345");
        var req = new HttpRequestMessage(HttpMethod.Post, "/upload")
        {
            Content = new ByteArrayContent(payload),
        };
        req.Headers.Add("x-networker-request-id", "test-id-123");
        var resp = await client.SendAsync(req);

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        var received = resp.Headers.GetValues("x-networker-received-bytes").First();
        Assert.Equal(payload.Length.ToString(), received);
        Assert.Equal("test-id-123", resp.Headers.GetValues("x-networker-request-id").First());

        var json = await resp.Content.ReadFromJsonAsync<JsonElement>();
        Assert.Equal(payload.Length, json.GetProperty("received_bytes").GetInt32());
    }

    [Fact]
    public async Task Echo_Post_Returns_Body_Verbatim()
    {
        var client = _factory.CreateClient();
        var payload = Encoding.UTF8.GetBytes("hello world");
        var resp = await client.PostAsync("/echo", new ByteArrayContent(payload));

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        Assert.Equal("application/octet-stream", resp.Content.Headers.ContentType?.MediaType);
        var body = await resp.Content.ReadAsByteArrayAsync();
        Assert.Equal(payload, body);
    }

    [Theory]
    [InlineData(404, HttpStatusCode.NotFound)]
    [InlineData(503, HttpStatusCode.ServiceUnavailable)]
    public async Task StatusCode_Returns_Requested(int code, HttpStatusCode expected)
    {
        var client = _factory.CreateClient();
        var resp = await client.GetAsync($"/status/{code}");
        Assert.Equal(expected, resp.StatusCode);
        var json = await resp.Content.ReadFromJsonAsync<JsonElement>();
        Assert.Equal(code, json.GetProperty("status").GetInt32());
    }

    [Fact]
    public async Task Landing_Returns_Html()
    {
        var client = _factory.CreateClient();
        var resp = await client.GetAsync("/");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        Assert.Contains("text/html", resp.Content.Headers.ContentType?.ToString());
        var html = await resp.Content.ReadAsStringAsync();
        Assert.Contains("networker-endpoint", html);
        Assert.Contains("/health", html);
        Assert.Contains(":8080", html);
    }

    [Fact]
    public async Task Info_Lists_Protocols_And_Endpoints()
    {
        var client = _factory.CreateClient();
        var json = await client.GetFromJsonAsync<JsonElement>("/info");
        Assert.Equal("networker-endpoint", json.GetProperty("service").GetString());
        var protocols = json.GetProperty("protocols").EnumerateArray().Select(p => p.GetString()).ToList();
        Assert.Contains("HTTP/1.1", protocols);
        Assert.Contains("HTTP/2", protocols);
        Assert.True(json.GetProperty("endpoints").GetArrayLength() >= 16);
        Assert.True(json.TryGetProperty("system", out _));
        Assert.True(json.TryGetProperty("uptime_secs", out _));
    }

    [Fact]
    public async Task Delay_Responds_With_Ms()
    {
        var client = _factory.CreateClient();
        var json = await client.GetFromJsonAsync<JsonElement>("/delay?ms=10");
        Assert.Equal(10, json.GetProperty("delayed_ms").GetInt32());
    }

    [Fact]
    public async Task Headers_Echoes_Request_Headers()
    {
        var client = _factory.CreateClient();
        var req = new HttpRequestMessage(HttpMethod.Get, "/headers");
        req.Headers.Add("x-test-header", "networker");
        var resp = await client.SendAsync(req);
        var json = await resp.Content.ReadFromJsonAsync<JsonElement>();
        Assert.Equal("networker", json.GetProperty("x-test-header").GetString());
    }

    [Fact]
    public async Task HttpVersion_Returns_Version_Field()
    {
        var client = _factory.CreateClient();
        var json = await client.GetFromJsonAsync<JsonElement>("/http-version");
        Assert.Equal(JsonValueKind.String, json.GetProperty("version").ValueKind);
    }

    // ── Page-load routes ─────────────────────────────────────────────────────

    [Fact]
    public async Task Page_Manifest_Lists_Assets()
    {
        var client = _factory.CreateClient();
        var json = await client.GetFromJsonAsync<JsonElement>("/page?assets=5&bytes=100");
        Assert.Equal(5, json.GetProperty("asset_count").GetInt32());
        Assert.Equal(100, json.GetProperty("asset_bytes").GetInt32());
        Assert.Equal(5, json.GetProperty("assets").GetArrayLength());
        Assert.Equal("/asset?id=0&bytes=100", json.GetProperty("assets")[0].GetString());
    }

    [Fact]
    public async Task BrowserPage_Returns_Html_With_Img_Tags()
    {
        var client = _factory.CreateClient();
        var resp = await client.GetAsync("/browser-page?assets=3&bytes=50");
        Assert.Contains("text/html", resp.Content.Headers.ContentType?.ToString());
        var html = await resp.Content.ReadAsStringAsync();
        Assert.Equal(3, System.Text.RegularExpressions.Regex.Matches(html, "<img ").Count);
    }

    [Fact]
    public async Task Asset_Returns_N_Bytes()
    {
        var client = _factory.CreateClient();
        var body = await client.GetByteArrayAsync("/asset?id=1&bytes=512");
        Assert.Equal(512, body.Length);
    }

    // ── JSON API benchmark endpoints ─────────────────────────────────────────

    [Fact]
    public async Task ApiUsers_Returns_20_With_Bench_Headers()
    {
        var client = _factory.CreateClient();
        var resp = await client.GetAsync("/api/users?page=1&sort=name&order=asc");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        Assert.Contains("no-store", resp.Headers.CacheControl?.ToString() ?? "");
        Assert.True(resp.Headers.Contains("timing-allow-origin"));
        Assert.True(resp.Headers.Contains("access-control-allow-origin"));
        var json = await resp.Content.ReadFromJsonAsync<JsonElement>();
        Assert.Equal(20, json.GetArrayLength());
    }

    [Fact]
    public async Task ApiUsers_Is_Deterministic()
    {
        var client = _factory.CreateClient();
        var b1 = await client.GetStringAsync("/api/users?page=5");
        var b2 = await client.GetStringAsync("/api/users?page=5");
        Assert.Equal(b1, b2);
    }

    [Fact]
    public async Task ApiTransform_Hashes_And_Reverses()
    {
        var client = _factory.CreateClient();
        var body = JsonContent.Create(new { seed = 1, fields = new[] { "hello", "world" }, values = new[] { 1, 2, 3 } });
        var resp = await client.PostAsync("/api/transform", body);
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        var json = await resp.Content.ReadFromJsonAsync<JsonElement>();
        var reversed = json.GetProperty("reversed_values").EnumerateArray().Select(x => x.GetInt32()).ToArray();
        Assert.Equal(new[] { 3, 2, 1 }, reversed);
        // SHA-256("hello")
        Assert.Equal(
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
            json.GetProperty("hashed_fields")[0].GetString());
    }

    [Fact]
    public async Task ApiAggregate_Returns_Stats()
    {
        var client = _factory.CreateClient();
        var json = await client.GetFromJsonAsync<JsonElement>("/api/aggregate?range=1,100");
        Assert.Equal(10_000, json.GetProperty("total_points").GetInt32());
        Assert.Equal(5, json.GetProperty("categories").GetArrayLength());
        Assert.True(json.GetProperty("mean").GetDouble() >= 0);
    }

    [Fact]
    public async Task ApiSearch_Returns_Results()
    {
        var client = _factory.CreateClient();
        var json = await client.GetFromJsonAsync<JsonElement>("/api/search?q=network&limit=5");
        Assert.True(json.GetProperty("total_matches").GetInt32() > 0);
        Assert.True(json.GetProperty("results").GetArrayLength() <= 5);
    }

    [Fact]
    public async Task ApiUploadProcess_Computes_Hashes()
    {
        var client = _factory.CreateClient();
        var payload = Encoding.UTF8.GetBytes("hello world benchmark test data");
        var resp = await client.PostAsync("/api/upload/process", new ByteArrayContent(payload));
        var json = await resp.Content.ReadFromJsonAsync<JsonElement>();
        Assert.Equal(payload.Length, json.GetProperty("original_size").GetInt32());
        Assert.True(json.GetProperty("compressed_size").GetInt32() > 0);
        Assert.Equal(8, json.GetProperty("crc32").GetString()!.Length);
        Assert.Equal(64, json.GetProperty("sha256").GetString()!.Length);
    }

    [Fact]
    public async Task ApiUploadProcess_Crc32_Matches_Known_Value()
    {
        var client = _factory.CreateClient();
        // CRC32("123456789") == 0xCBF43926 (standard IEEE test vector).
        var resp = await client.PostAsync("/api/upload/process",
            new ByteArrayContent(Encoding.ASCII.GetBytes("123456789")));
        var json = await resp.Content.ReadFromJsonAsync<JsonElement>();
        Assert.Equal("cbf43926", json.GetProperty("crc32").GetString());
    }

    [Fact]
    public async Task ApiDelayed_Clamps_To_100()
    {
        var client = _factory.CreateClient();
        var json = await client.GetFromJsonAsync<JsonElement>("/api/delayed?ms=999");
        Assert.Equal(100, json.GetProperty("requested_ms").GetInt32());
    }

    [Fact]
    public async Task ApiValidate_Returns_Checksums()
    {
        var client = _factory.CreateClient();
        var json = await client.GetFromJsonAsync<JsonElement>("/api/validate?seed=42");
        Assert.Equal(42, json.GetProperty("seed").GetInt32());
        var checksums = json.GetProperty("checksums");
        Assert.Equal(64, checksums.GetProperty("users").GetString()!.Length);
        Assert.Equal(64, checksums.GetProperty("aggregate").GetString()!.Length);
        Assert.Equal(64, checksums.GetProperty("transform").GetString()!.Length);
        Assert.Equal(64, checksums.GetProperty("search").GetString()!.Length);
    }

    [Fact]
    public async Task ApiEndpoints_Include_Auth_Timing_And_Bench_Headers()
    {
        var client = _factory.CreateClient();
        var resp = await client.GetAsync("/api/users?page=1&sort=name&order=asc");
        var st = ServerTiming(resp);
        Assert.Contains("auth;dur=", st);
        Assert.Contains("app;dur=", st);
    }

    private static string ServerTiming(HttpResponseMessage resp)
    {
        if (resp.Headers.TryGetValues("server-timing", out var v)) return string.Join(", ", v);
        if (resp.Content.Headers.TryGetValues("server-timing", out var cv)) return string.Join(", ", cv);
        return "";
    }
}
