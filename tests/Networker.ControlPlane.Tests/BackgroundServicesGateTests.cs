using Networker.ControlPlane.Background;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins the DASHBOARD_BACKGROUND_SERVICES parse rules (the single-replica safety
/// gate): only "0"/"false" disable the hosted background loops; everything else
/// — most importantly UNSET — keeps them enabled so single-replica deployments
/// need no new configuration.
/// </summary>
public class BackgroundServicesGateTests
{
    [Theory]
    [InlineData(null, true)]      // unset → default enabled
    [InlineData("", true)]        // empty → default enabled
    [InlineData("1", true)]
    [InlineData("true", true)]
    [InlineData("yes", true)]     // unknown values keep the default
    [InlineData("0", false)]
    [InlineData(" 0 ", false)]
    [InlineData("false", false)]
    [InlineData("False", false)]
    [InlineData("FALSE", false)]
    [InlineData(" false ", false)]
    public void ParseEnabled_OnlyZeroOrFalseDisable(string? raw, bool expected)
    {
        Assert.Equal(expected, BackgroundServicesGate.ParseEnabled(raw));
    }
}
