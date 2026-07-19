using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins <see cref="RunVerdict.ResultStatus"/> — the server-side twin of the
/// frontend's <c>runDisplayStatus()</c> (<c>dashboard/src/lib/runStatus.ts</c>,
/// audit F9). The two implement the SAME verdict rule; these cases mirror
/// <c>runStatus.test.ts</c> so a change to one side fails the other's pin.
/// </summary>
public sealed class RunVerdictTests
{
    [Fact]
    public void Completed_with_zero_failures_is_completed()
    {
        Assert.Equal("completed", RunVerdict.ResultStatus("completed", 9, 0));
        Assert.Equal("completed", RunVerdict.ResultStatus("completed", 0, 0));
    }

    [Fact]
    public void Completed_with_mixed_results_is_partial()
    {
        Assert.Equal("partial", RunVerdict.ResultStatus("completed", 7, 2));
        Assert.Equal("partial", RunVerdict.ResultStatus("completed", 1, 99));
    }

    [Fact]
    public void Completed_where_everything_failed_is_failed()
    {
        Assert.Equal("failed", RunVerdict.ResultStatus("completed", 0, 9));
    }

    [Theory]
    [InlineData("running")]
    [InlineData("queued")]
    [InlineData("provisioning")]
    [InlineData("failed")]
    [InlineData("cancelled")]
    public void Non_completed_statuses_pass_through_verbatim(string status)
    {
        // Counters are irrelevant for non-completed lifecycle states — a
        // running run with failures so far is still "running".
        Assert.Equal(status, RunVerdict.ResultStatus(status, 3, 2));
    }
}
