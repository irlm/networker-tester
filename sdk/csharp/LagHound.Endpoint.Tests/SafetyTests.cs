using System.Net;
using System.Text.Json;
using Xunit;

namespace LagHound.Endpoint.Tests;

/// <summary>
/// Production-safety properties (contract v1 §5, §6): 404-invisibility,
/// rate-limit-before-auth, kill switch, concurrency + transfer caps, byte
/// budget, streaming memory bound, and auth acceptance (Bearer, rotation).
/// </summary>
[Collection("laghound-env")] // serialize the env-var-mutating tests
public sealed class SafetyTests
{
    [Fact]
    public async Task Missing_Token_Is_Bare_404_On_All_Routes_Including_Health()
    {
        using var host = await TestHost.StartAsync();
        foreach (var path in new[] { "/laghound/health", "/laghound/echo", "/laghound/download", "/laghound/info" })
        {
            var res = await TestHost.Client(host).GetAsync(path);
            Assert.Equal(HttpStatusCode.NotFound, res.StatusCode);
            Assert.False(res.Headers.Contains("Server-Timing"));
            Assert.False(res.Headers.Contains("WWW-Authenticate"));
        }
    }

    [Fact]
    public async Task Bad_Token_Is_Bare_404()
    {
        using var host = await TestHost.StartAsync();
        var req = new HttpRequestMessage(HttpMethod.Get, "/laghound/health");
        req.Headers.Add("X-LagHound-Token", "wrong-token-0123456789");
        var res = await TestHost.Client(host).SendAsync(req);
        Assert.Equal(HttpStatusCode.NotFound, res.StatusCode);
        Assert.False(res.Headers.Contains("Server-Timing"));
    }

    [Fact]
    public async Task Bare_404_For_Bad_Token_Matches_HostApp_404()
    {
        using var host = await TestHost.StartAsync();
        var badToken = new HttpRequestMessage(HttpMethod.Get, "/laghound/health");
        badToken.Headers.Add("X-LagHound-Token", "wrong-token-0123456789");
        var lag = await TestHost.Client(host).SendAsync(badToken);
        var appMiss = await TestHost.Client(host).GetAsync("/definitely-not-a-route");

        Assert.Equal(appMiss.StatusCode, lag.StatusCode);
        Assert.Equal(
            (await appMiss.Content.ReadAsStringAsync()).Length,
            (await lag.Content.ReadAsStringAsync()).Length);
    }

    [Fact]
    public async Task Bearer_Header_Authenticates()
    {
        using var host = await TestHost.StartAsync();
        var res = await TestHost.Client(host).SendAsync(TestHost.Bearer(HttpMethod.Get, "/laghound/health", TestHost.Token));
        Assert.Equal(HttpStatusCode.OK, res.StatusCode);
    }

    [Fact]
    public async Task XLagHoundToken_Wins_Over_Bearer()
    {
        using var host = await TestHost.StartAsync();
        var req = new HttpRequestMessage(HttpMethod.Get, "/laghound/health");
        req.Headers.Add("X-LagHound-Token", TestHost.Token);       // correct
        req.Headers.Add("Authorization", "Bearer garbage-token-xx"); // ignored
        var res = await TestHost.Client(host).SendAsync(req);
        Assert.Equal(HttpStatusCode.OK, res.StatusCode);
    }

    [Fact]
    public async Task Previous_Token_Accepted_For_Rotation()
    {
        const string prev = "previous-token-0123456789";
        using var host = await TestHost.StartAsync(o => o.PreviousToken = prev);
        var res = await TestHost.Client(host).SendAsync(TestHost.Bearer(HttpMethod.Get, "/laghound/health", prev));
        Assert.Equal(HttpStatusCode.OK, res.StatusCode);
    }

    [Fact]
    public async Task Mount_Without_Token_Fails_Closed()
    {
        // No Token and no LAGHOUND_TOKEN env → AddLagHound must throw.
        var prior = Environment.GetEnvironmentVariable("LAGHOUND_TOKEN");
        Environment.SetEnvironmentVariable("LAGHOUND_TOKEN", null);
        try
        {
            await Assert.ThrowsAnyAsync<Exception>(() => TestHost.StartAsync(o => o.Token = null));
        }
        finally
        {
            Environment.SetEnvironmentVariable("LAGHOUND_TOKEN", prior);
        }
    }

    [Fact]
    public async Task Short_Token_Rejected()
    {
        await Assert.ThrowsAnyAsync<Exception>(() => TestHost.StartAsync(o => o.Token = "short"));
    }

    [Fact]
    public async Task KillSwitch_Makes_Everything_Bare_404()
    {
        Environment.SetEnvironmentVariable("LAGHOUND_DISABLED", "1");
        LagHound.Endpoint.Internal.KillSwitch.Refresh();
        try
        {
            using var host = await TestHost.StartAsync();
            var res = await TestHost.Client(host).SendAsync(TestHost.Authed(HttpMethod.Get, "/laghound/health"));
            Assert.Equal(HttpStatusCode.NotFound, res.StatusCode);
            Assert.False(res.Headers.Contains("Server-Timing"));
        }
        finally
        {
            Environment.SetEnvironmentVariable("LAGHOUND_DISABLED", null);
            LagHound.Endpoint.Internal.KillSwitch.Refresh();
        }
    }

    [Fact]
    public async Task Unauthenticated_RateLimit_Is_Bare_404_Not_429()
    {
        // Tiny per-IP bucket; unauthenticated flood must yield bare 404s, never 429.
        using var host = await TestHost.StartAsync(o =>
        {
            o.RatePerIpRps = 1;
            o.RatePerIpBurst = 1;
        });
        var client = TestHost.Client(host);
        var statuses = new List<HttpStatusCode>();
        for (int i = 0; i < 10; i++)
        {
            var res = await client.GetAsync("/laghound/health"); // no token
            statuses.Add(res.StatusCode);
        }

        Assert.All(statuses, s => Assert.Equal(HttpStatusCode.NotFound, s));
        Assert.DoesNotContain(HttpStatusCode.TooManyRequests, statuses);
    }

    [Fact]
    public async Task Authenticated_RateLimit_Is_429_With_RetryAfter()
    {
        using var host = await TestHost.StartAsync(o =>
        {
            o.RatePerIpRps = 1;
            o.RatePerIpBurst = 1;
        });
        var client = TestHost.Client(host);
        HttpResponseMessage? limited = null;
        for (int i = 0; i < 10; i++)
        {
            var res = await client.SendAsync(TestHost.Authed(HttpMethod.Get, "/laghound/health"));
            if (res.StatusCode == HttpStatusCode.TooManyRequests)
            {
                limited = res;
                break;
            }
        }

        Assert.NotNull(limited);
        Assert.True(limited!.Headers.Contains("Retry-After"));
        using var doc = JsonDocument.Parse(await limited.Content.ReadAsStringAsync());
        Assert.Equal("rate_limited", doc.RootElement.GetProperty("error").GetProperty("code").GetString());
        Assert.True(doc.RootElement.GetProperty("error").GetProperty("retry_after_ms").GetInt64() > 0);
    }

    [Fact]
    public async Task Transfer_Concurrency_Cap_Rejects_Second_Then_Frees_The_Slot()
    {
        // max_concurrent_transfers=1: a second concurrent transfer is rejected
        // 429, and the slot is released once the first drains.
        using var host = await TestHost.StartAsync(o =>
        {
            o.MaxConcurrentTransfers = 1;
            o.DownloadCapBytes = 8 * 1024 * 1024;
            o.RatePerIpRps = 1000;
            o.RatePerIpBurst = 1000;
            o.RateGlobalRps = 1000;
            o.RateGlobalBurst = 1000;
        });
        var client = TestHost.Client(host);

        // Deterministic slot ordering: the transfer gate is acquired BEFORE the
        // 200 is written (LagHoundMiddleware.DownloadAsync), and the Lease is held
        // until the body finishes streaming. Awaiting the first response's HEADERS
        // therefore proves the single slot is held; leaving the 8 MiB body
        // undrained keeps the writer paused on backpressure, so the slot stays
        // held for the duration of the assertion — no timing/scheduling race.
        var first = await client.SendAsync(
            TestHost.Authed(HttpMethod.Get, "/laghound/download?bytes=8388608"),
            HttpCompletionOption.ResponseHeadersRead);
        Assert.Equal(HttpStatusCode.OK, first.StatusCode);

        var second = await client.SendAsync(
            TestHost.Authed(HttpMethod.Get, "/laghound/download?bytes=8388608"),
            HttpCompletionOption.ResponseHeadersRead);
        Assert.Equal(HttpStatusCode.TooManyRequests, second.StatusCode);

        // Fully draining the first returns from DownloadAsync → disposes the
        // Lease → frees the slot, so the next transfer now succeeds. This proves
        // the cap RELEASES, not just that it rejects (a cap that never frees would
        // also pass the assertion above).
        await first.Content.ReadAsByteArrayAsync();
        var third = await client.SendAsync(
            TestHost.Authed(HttpMethod.Get, "/laghound/download?bytes=8388608"),
            HttpCompletionOption.ResponseHeadersRead);
        Assert.Equal(HttpStatusCode.OK, third.StatusCode);
        await third.Content.ReadAsByteArrayAsync();
    }

    [Fact]
    public async Task Byte_Budget_Exhaustion_Returns_429_RetryAfter()
    {
        using var host = await TestHost.StartAsync(o =>
        {
            o.ByteBudgetBytes = 512 * 1024; // 512 KiB budget
            o.ByteBudgetWindowSeconds = 600;
            o.DownloadCapBytes = 256 * 1024;
            o.RatePerIpRps = 1000;
            o.RatePerIpBurst = 1000;
            o.RateGlobalRps = 1000;
            o.RateGlobalBurst = 1000;
        });
        var client = TestHost.Client(host);

        HttpResponseMessage? budgeted = null;
        for (int i = 0; i < 20; i++)
        {
            var res = await client.SendAsync(TestHost.Authed(HttpMethod.Get, "/laghound/download?bytes=262144"));
            if (res.StatusCode == HttpStatusCode.TooManyRequests)
            {
                budgeted = res;
                break;
            }

            await res.Content.ReadAsByteArrayAsync();
        }

        Assert.NotNull(budgeted);
        Assert.True(budgeted!.Headers.Contains("Retry-After"));
        using var doc = JsonDocument.Parse(await budgeted.Content.ReadAsStringAsync());
        Assert.Equal("rate_limited", doc.RootElement.GetProperty("error").GetProperty("code").GetString());
    }

    [Fact]
    public async Task Large_Download_Does_Not_Balloon_Memory()
    {
        // Stream a 32 MiB download under the abs-max config; RSS-ish proxy:
        // managed heap growth must stay well under the payload size, proving the
        // body is streamed from the shared buffer (contract §9).
        using var host = await TestHost.StartAsync(o =>
        {
            o.DownloadCapBytes = 32 * 1024 * 1024;
            o.RatePerIpRps = 1000;
            o.RatePerIpBurst = 1000;
            o.RateGlobalRps = 1000;
            o.RateGlobalBurst = 1000;
        });

        GC.Collect();
        GC.WaitForPendingFinalizers();
        GC.Collect();
        long before = GC.GetTotalAllocatedBytes(precise: true);

        var res = await TestHost.Client(host).SendAsync(
            TestHost.Authed(HttpMethod.Get, "/laghound/download?bytes=33554432"),
            HttpCompletionOption.ResponseHeadersRead);
        Assert.Equal(HttpStatusCode.OK, res.StatusCode);
        Assert.Equal(33554432L, res.Content.Headers.ContentLength);

        // Drain server-side without keeping the whole body: copy to Null.
        await using var stream = await res.Content.ReadAsStreamAsync();
        await stream.CopyToAsync(Stream.Null);

        long after = GC.GetTotalAllocatedBytes(precise: true);
        long serverSide = after - before;
        // The client-side stream copy itself allocates; we assert the total is
        // not proportional to a *buffered* 32 MiB body (would be >= 32 MiB).
        Assert.True(serverSide < 32L * 1024 * 1024,
            $"allocations {serverSide} bytes should be < 32 MiB (streamed, not buffered)");
    }
}

/// <summary>Serializes tests that mutate process-wide environment variables.</summary>
[CollectionDefinition("laghound-env")]
public sealed class EnvCollection { }
