using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Unit tests for <see cref="ProvisioningOrchestrator.SanitizeVmLabel"/>. A
/// comparison-group cell named "rust @ nginx …" produced the VM name
/// "nwk-auto-rust--" (trailing dashes from "@ " → "--" then a 15-char
/// truncation landing on a dash). Azure derives the NIC ipconfig as
/// `ipconfig<vmName>`, which must END with a word char — so that VM failed to
/// create with InvalidResourceName and took the whole deploy down (install.sh
/// exit 1; E2E 2026-07-29). These lock the invariant: never a trailing/leading
/// dash, dash-runs collapsed, always Azure-valid.
/// </summary>
public class SanitizeVmLabelTests
{
    [Theory]
    // The exact regression: "@ " → "rust--" then truncation → trailing dash.
    [InlineData("nwk-auto-rust--ng", "nwk-auto-rust-n")]
    // Trailing dash from the raw input.
    [InlineData("nwk-auto-go-", "nwk-auto-go")]
    // Dash run collapses.
    [InlineData("nwk-auto-a---b", "nwk-auto-a-b")]
    // Already clean, under the cap.
    [InlineData("nwk-auto-go", "nwk-auto-go")]
    public void Produces_azure_valid_labels(string raw, string expected)
        => Assert.Equal(expected, ProvisioningOrchestrator.SanitizeVmLabel(raw));

    [Theory]
    [InlineData("nwk-auto-rust--ng")]
    [InlineData("nwk-auto-python @ nginx · cg-abcd1234·0")]
    [InlineData("nwk-auto----")]
    [InlineData("a-b-c-d-e-f-g-h-i-j")]  // forces truncation mid-dash
    public void Never_starts_or_ends_with_a_dash_and_is_bounded(string raw)
    {
        var label = ProvisioningOrchestrator.SanitizeVmLabel(raw);
        Assert.True(label.Length is > 0 and <= 15, $"len {label.Length}");
        Assert.DoesNotContain("--", label);          // no dash runs
        Assert.False(label.StartsWith('-'), "leading dash");
        Assert.False(label.EndsWith('-'), "trailing dash");
        // The invariant that actually failed Azure: ipconfig<name> ends word-char.
        Assert.Matches("[a-z0-9]$", label);
    }

    [Fact]
    public void All_separators_falls_back_rather_than_empty()
        => Assert.Equal("nwk-auto-vm", ProvisioningOrchestrator.SanitizeVmLabel("----"));
}
