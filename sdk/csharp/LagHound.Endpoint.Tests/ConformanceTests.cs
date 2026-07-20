using System.Net;
using System.Text;
using System.Text.Json;
using Xunit;

namespace LagHound.Endpoint.Tests;

/// <summary>
/// Contract-v1 conformance: every assertion is derived from
/// shared/sdk-contract-v1.json (loaded via <see cref="ContractModel"/>) and
/// checked against a live in-memory host. Route shapes, caps, headers, and
/// statuses all trace back to the machine-readable contract.
/// </summary>
public sealed class ConformanceTests
{
    private static readonly ContractModel Contract = ContractModel.Load();

    [Fact]
    public void Contract_Is_V1()
    {
        Assert.Equal("v1", Contract.Root.GetProperty("contract").GetString());
        Assert.Equal("/laghound", Contract.Root.GetProperty("prefix_default").GetString());
    }

    [Fact]
    public async Task Health_Returns_Contract_Shape()
    {
        using var host = await TestHost.StartAsync(o => o.AppName = "checkout-api");
        var res = await TestHost.Client(host).SendAsync(TestHost.Authed(HttpMethod.Get, "/laghound/health"));

        Assert.Equal(HttpStatusCode.OK, res.StatusCode);
        AssertCommonSuccessHeaders(res);
        using var body = JsonDocument.Parse(await res.Content.ReadAsStringAsync());
        var root = body.RootElement;
        Assert.Equal("v1", root.GetProperty("contract").GetString());
        Assert.Equal("ok", root.GetProperty("status").GetString());
        Assert.Equal("csharp", root.GetProperty("sdk").GetProperty("lang").GetString());
        Assert.False(string.IsNullOrEmpty(root.GetProperty("sdk").GetProperty("version").GetString()));
        Assert.Equal("checkout-api", root.GetProperty("app").GetString());
        Assert.True(root.GetProperty("uptime_s").GetInt64() >= 0);
        var routes = root.GetProperty("routes");
        Assert.True(routes.GetProperty("health").GetBoolean());
        Assert.True(routes.GetProperty("echo").GetBoolean());
        Assert.True(routes.GetProperty("download").GetBoolean());
        Assert.True(routes.GetProperty("upload").GetBoolean());
        Assert.True(routes.GetProperty("info").GetBoolean());
    }

    [Fact]
    public async Task Echo_Returns_Fixed_Body_And_ServerTiming_App()
    {
        var expected = Contract.Route("echo").GetProperty("response").GetProperty("body_fixed");
        string wantContract = expected.GetProperty("contract").GetString()!;
        bool wantOk = expected.GetProperty("ok").GetBoolean();

        using var host = await TestHost.StartAsync();
        var res = await TestHost.Client(host).SendAsync(TestHost.Authed(HttpMethod.Get, "/laghound/echo"));

        Assert.Equal(HttpStatusCode.OK, res.StatusCode);
        AssertCommonSuccessHeaders(res);
        Assert.Contains("app;dur=", ServerTiming(res));
        string body = await res.Content.ReadAsStringAsync();
        Assert.True(Encoding.UTF8.GetByteCount(body) < 1024, "echo body must be < 1 KiB");
        using var doc = JsonDocument.Parse(body);
        Assert.Equal(wantContract, doc.RootElement.GetProperty("contract").GetString());
        Assert.Equal(wantOk, doc.RootElement.GetProperty("ok").GetBoolean());
    }

    [Fact]
    public async Task Echo_Body_Is_Byte_Constant_Across_Requests()
    {
        using var host = await TestHost.StartAsync();
        var a = await (await TestHost.Client(host).SendAsync(TestHost.Authed(HttpMethod.Get, "/laghound/echo"))).Content.ReadAsByteArrayAsync();
        var b = await (await TestHost.Client(host).SendAsync(TestHost.Authed(HttpMethod.Get, "/laghound/echo"))).Content.ReadAsByteArrayAsync();
        Assert.Equal(a, b);
    }

    [Fact]
    public async Task Download_Default_Size_Matches_Contract()
    {
        long def = Contract.Cap("download_default_bytes");
        using var host = await TestHost.StartAsync();
        var res = await TestHost.Client(host).SendAsync(TestHost.Authed(HttpMethod.Get, "/laghound/download"));

        Assert.Equal(HttpStatusCode.OK, res.StatusCode);
        Assert.Equal("application/octet-stream", res.Content.Headers.ContentType!.MediaType);
        Assert.Equal(def, res.Content.Headers.ContentLength);
        Assert.Equal(def.ToString(), XBytes(res));
        var bytes = await res.Content.ReadAsByteArrayAsync();
        Assert.Equal(def, bytes.LongLength);
        Assert.All(bytes, b => Assert.Equal((byte)Contract.Route("download").GetProperty("response").GetProperty("fill_byte").GetInt32(), b));
    }

    [Fact]
    public async Task Download_Clamps_To_Cap_And_Reports_Actual()
    {
        // Configure a small cap; ask for far more; expect clamp + X-LagHound-Bytes.
        long cap = 128 * 1024;
        using var host = await TestHost.StartAsync(o => o.DownloadCapBytes = cap);
        var res = await TestHost.Client(host).SendAsync(TestHost.Authed(HttpMethod.Get, "/laghound/download?bytes=99999999"));

        Assert.Equal(HttpStatusCode.OK, res.StatusCode);
        Assert.Equal(cap, res.Content.Headers.ContentLength);
        Assert.Equal(cap.ToString(), XBytes(res));
    }

    [Fact]
    public async Task Download_Respects_Absolute_Max()
    {
        long absMax = Contract.Cap("absolute_max_bytes");
        // Ask config for more than the absolute max; it must clamp to absMax.
        using var host = await TestHost.StartAsync(o => o.DownloadCapBytes = absMax + 10_000_000);
        var res = await TestHost.Client(host).SendAsync(TestHost.Authed(HttpMethod.Get, $"/laghound/download?bytes={absMax + 10_000_000}"));
        Assert.Equal(absMax, res.Content.Headers.ContentLength);
        Assert.Equal(absMax.ToString(), XBytes(res));
    }

    [Fact]
    public async Task Download_Invalid_Bytes_Is_400_Envelope()
    {
        using var host = await TestHost.StartAsync();
        var res = await TestHost.Client(host).SendAsync(TestHost.Authed(HttpMethod.Get, "/laghound/download?bytes=abc"));
        await AssertEnvelope(res, HttpStatusCode.BadRequest, "invalid_param");
    }

    [Fact]
    public async Task Download_Negative_Bytes_Is_400()
    {
        using var host = await TestHost.StartAsync();
        var res = await TestHost.Client(host).SendAsync(TestHost.Authed(HttpMethod.Get, "/laghound/download?bytes=-5"));
        await AssertEnvelope(res, HttpStatusCode.BadRequest, "invalid_param");
    }

    [Fact]
    public async Task Upload_Counts_Bytes_And_Reports_Header()
    {
        using var host = await TestHost.StartAsync();
        var payload = new byte[100_000];
        var req = TestHost.Authed(HttpMethod.Post, "/laghound/upload");
        req.Content = new ByteArrayContent(payload);
        var res = await TestHost.Client(host).SendAsync(req);

        Assert.Equal(HttpStatusCode.OK, res.StatusCode);
        Assert.Equal(payload.Length.ToString(), XBytes(res));
        Assert.Contains("recv;dur=", ServerTiming(res));
        Assert.Contains("app;dur=", ServerTiming(res));
        using var doc = JsonDocument.Parse(await res.Content.ReadAsStringAsync());
        Assert.Equal("v1", doc.RootElement.GetProperty("contract").GetString());
        Assert.Equal(payload.Length, doc.RootElement.GetProperty("received_bytes").GetInt64());
    }

    [Fact]
    public async Task Upload_ContentLength_Over_Cap_Is_413_Without_Reading()
    {
        long cap = 64 * 1024;
        using var host = await TestHost.StartAsync(o => o.UploadCapBytes = cap);
        var req = TestHost.Authed(HttpMethod.Post, "/laghound/upload");
        req.Content = new ByteArrayContent(new byte[cap + 50_000]); // Content-Length set
        var res = await TestHost.Client(host).SendAsync(req);
        await AssertEnvelope(res, HttpStatusCode.RequestEntityTooLarge, "payload_too_large");
    }

    [Fact]
    public async Task Info_Echoes_Config_Without_Token()
    {
        using var host = await TestHost.StartAsync(o => o.AppName = "checkout-api");
        var res = await TestHost.Client(host).SendAsync(TestHost.Authed(HttpMethod.Get, "/laghound/info"));

        Assert.Equal(HttpStatusCode.OK, res.StatusCode);
        AssertCommonSuccessHeaders(res);
        string body = await res.Content.ReadAsStringAsync();
        Assert.DoesNotContain(TestHost.Token, body);
        using var doc = JsonDocument.Parse(body);
        var root = doc.RootElement;
        Assert.Equal("v1", root.GetProperty("contract").GetString());
        Assert.Equal("/laghound", root.GetProperty("prefix").GetString());
        Assert.True(root.GetProperty("token_set").GetBoolean());
        Assert.Equal(Contract.Cap("download_default_bytes"), root.GetProperty("caps").GetProperty("download_bytes").GetInt64());
        Assert.Equal(Contract.Cap("upload_default_bytes"), root.GetProperty("caps").GetProperty("upload_bytes").GetInt64());
        Assert.Equal(Contract.Cap("absolute_max_bytes"), root.GetProperty("caps").GetProperty("absolute_max_bytes").GetInt64());
        var limits = root.GetProperty("limits");
        Assert.Equal(8, limits.GetProperty("max_concurrent").GetInt32());
        Assert.Equal(2, limits.GetProperty("max_concurrent_transfers").GetInt32());
        Assert.Equal(JsonValueKind.Null, limits.GetProperty("byte_budget").ValueKind);
    }

    [Fact]
    public async Task MethodNotAllowed_On_Known_Route()
    {
        using var host = await TestHost.StartAsync();
        // POST to /echo (a GET route), authenticated → 405 envelope.
        var res = await TestHost.Client(host).SendAsync(TestHost.Authed(HttpMethod.Post, "/laghound/echo"));
        await AssertEnvelope(res, (HttpStatusCode)405, "method_not_allowed");
    }

    [Fact]
    public async Task Unknown_Subpath_Under_Prefix_Is_Bare_404()
    {
        using var host = await TestHost.StartAsync();
        var res = await TestHost.Client(host).SendAsync(TestHost.Authed(HttpMethod.Get, "/laghound/nope"));
        await AssertBare404(res);
    }

    private static string ServerTiming(HttpResponseMessage res)
        => res.Headers.TryGetValues("Server-Timing", out var v) ? string.Join(",", v) : string.Empty;

    private static string XBytes(HttpResponseMessage res)
        => res.Headers.TryGetValues("X-LagHound-Bytes", out var v) ? string.Join("", v) : string.Empty;

    private static void AssertCommonSuccessHeaders(HttpResponseMessage res)
    {
        Assert.True(res.Headers.Contains("Server-Timing"), "success responses carry Server-Timing");
        Assert.Contains("app;dur=", ServerTiming(res));
        Assert.True(res.Headers.CacheControl!.NoStore, "Cache-Control must be no-store");
        Assert.True(res.Headers.Contains("Timing-Allow-Origin"));
    }

    private static async Task AssertEnvelope(HttpResponseMessage res, HttpStatusCode status, string code)
    {
        Assert.Equal(status, res.StatusCode);
        using var doc = JsonDocument.Parse(await res.Content.ReadAsStringAsync());
        Assert.Equal("v1", doc.RootElement.GetProperty("contract").GetString());
        Assert.Equal(code, doc.RootElement.GetProperty("error").GetProperty("code").GetString());
        Assert.False(string.IsNullOrEmpty(doc.RootElement.GetProperty("error").GetProperty("message").GetString()));
    }

    private static async Task AssertBare404(HttpResponseMessage res)
    {
        Assert.Equal(HttpStatusCode.NotFound, res.StatusCode);
        // No LagHound headers, no envelope, no Server-Timing.
        Assert.False(res.Headers.Contains("Server-Timing"));
        Assert.False(res.Headers.Contains("X-LagHound-Bytes"));
        string body = await res.Content.ReadAsStringAsync();
        Assert.DoesNotContain("contract", body);
    }
}
