using System.Net;
using System.Text;
using LagHound.Endpoint;
using LagHound.Endpoint.Internal;
using Microsoft.AspNetCore.Http;
using Xunit;

namespace LagHound.Endpoint.Tests;

/// <summary>
/// Focused unit tests for the internal helpers reachable via InternalsVisibleTo:
/// constant-time token compare (reviewed, per §9), Server-Timing header shape,
/// and the marks API.
/// </summary>
public sealed class UnitTests
{
    [Fact]
    public void ServerTiming_Emits_App_And_Total_Within_Limits()
    {
        string h = ServerTimingHeader.Build(new[] { ("app", 12.3456), ("total", 12.3456) });
        Assert.Contains("app;dur=12.346", h); // 3-decimal rounding
        Assert.Contains("total;dur=12.346", h);
        Assert.True(Encoding.UTF8.GetByteCount(h) <= 512);
    }

    [Fact]
    public void ServerTiming_Caps_At_Eight_Metrics()
    {
        var metrics = new (string, double)[10];
        for (int i = 0; i < 10; i++)
        {
            metrics[i] = ($"m{i}", i);
        }

        string h = ServerTimingHeader.Build(metrics);
        int count = h.Split(',').Length;
        Assert.True(count <= 8, $"expected <= 8 metrics, got {count}");
    }

    [Fact]
    public void ServerTiming_Negative_Duration_Clamped_To_Zero()
    {
        string h = ServerTimingHeader.Build(new[] { ("app", -5.0) });
        Assert.Contains("app;dur=0", h);
    }

    [Fact]
    public void Marks_Added_Via_Api_Surface_As_MarkPrefix()
    {
        var ctx = new DefaultHttpContext();
        LagHoundMarks.Mark(ctx, "db", TimeSpan.FromMilliseconds(41.9));
        LagHoundMarks.Mark(ctx, "cache", TimeSpan.FromMilliseconds(2.0));
        var marks = LagHoundMarks.Get(ctx);
        Assert.NotNull(marks);
        Assert.Equal(2, marks!.Count);

        string h = ServerTimingHeader.Build(new[] { ("app", 50.0) }, marks);
        Assert.Contains("mark-db;dur=41.9", h);
        Assert.Contains("mark-cache;dur=2", h);
    }

    [Theory]
    [InlineData("BadName!")]
    [InlineData("UPPER")]
    [InlineData("")]
    [InlineData("way-too-long-a-name-that-exceeds-24")]
    public void Marks_Reject_Invalid_Names(string name)
    {
        var ctx = new DefaultHttpContext();
        LagHoundMarks.Mark(ctx, name, TimeSpan.FromMilliseconds(1));
        Assert.Null(LagHoundMarks.Get(ctx));
    }

    [Fact]
    public void Token_Compare_Is_Length_Safe()
    {
        // Constant-time compare must not throw or short-circuit on length diff.
        var rt = new LagHoundRuntime(new LagHoundOptions { Token = "correct-token-0123456789" });
        Assert.True(rt.TokenMatches(Encoding.UTF8.GetBytes("correct-token-0123456789")));
        Assert.False(rt.TokenMatches(Encoding.UTF8.GetBytes("x")));            // shorter
        Assert.False(rt.TokenMatches(Encoding.UTF8.GetBytes("wrong-token-of-the-same-length!!")));
        Assert.False(rt.TokenMatches(ReadOnlySpan<byte>.Empty));               // empty
    }

    [Fact]
    public void Prefix_Validation_Rejects_Bad_Shapes()
    {
        Assert.Throws<ArgumentException>(() => new LagHoundRuntime(new LagHoundOptions { Token = "correct-token-0123456789", Prefix = "laghound" }));
        Assert.Throws<ArgumentException>(() => new LagHoundRuntime(new LagHoundOptions { Token = "correct-token-0123456789", Prefix = "/laghound/" }));
    }
}
