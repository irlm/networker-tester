using Microsoft.AspNetCore.Http;
using Networker.ControlPlane.Realtime.RawWs;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Agent api-key transport: the key now travels in the
/// <see cref="AgentSocketEndpoint.ApiKeyHeader"/> request header (so it never
/// lands in the URL / nginx access log), with the legacy <c>?key=</c> query kept
/// as a fallback for fielded pre-header agents until the Rust-agent decommission.
/// <see cref="AgentSocketEndpoint.ResolveAgentApiKey"/> is the extraction: header
/// preferred, query fallback. A regression here breaks agent auth (prod-critical),
/// so pin the precedence and the empty case.
/// </summary>
public sealed class ResolveAgentApiKeyTests
{
    private static HttpContext Ctx(string? header, string? query)
    {
        var http = new DefaultHttpContext();
        if (header is not null) http.Request.Headers[AgentSocketEndpoint.ApiKeyHeader] = header;
        if (query is not null) http.Request.QueryString = new QueryString($"?key={query}");
        return http;
    }

    [Fact]
    public void Prefers_the_header_over_the_query()
    {
        Assert.Equal("hdr-key", AgentSocketEndpoint.ResolveAgentApiKey(Ctx("hdr-key", "qry-key")));
    }

    [Fact]
    public void Uses_the_header_alone()
    {
        Assert.Equal("hdr-key", AgentSocketEndpoint.ResolveAgentApiKey(Ctx("hdr-key", null)));
    }

    [Fact]
    public void Falls_back_to_the_query_when_no_header()
    {
        Assert.Equal("qry-key", AgentSocketEndpoint.ResolveAgentApiKey(Ctx(null, "qry-key")));
    }

    [Fact]
    public void Blank_header_falls_back_to_the_query()
    {
        // An empty header must not shadow a present ?key= (fielded agent case).
        Assert.Equal("qry-key", AgentSocketEndpoint.ResolveAgentApiKey(Ctx("", "qry-key")));
    }

    [Fact]
    public void Neither_present_returns_empty_not_null()
    {
        // AuthenticateAsync treats empty as "no key" → 401; must never throw.
        Assert.Equal(string.Empty, AgentSocketEndpoint.ResolveAgentApiKey(Ctx(null, null)));
    }
}
