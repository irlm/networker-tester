using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The age→status rules behind the secret panel, and the invariants of the
/// inventory it renders. Pure, so the boundaries are pinned here rather than
/// eyeballed in the UI — an off-by-one at the "due" edge is exactly the kind of
/// thing that silently never warns.
/// </summary>
public class SecretAgeStatusTests
{
    [Theory]
    // never rotated — the case that matters most, and the one a design keyed off
    // the rotation table alone would hide entirely.
    [InlineData(null, 90, "never")]
    // comfortably inside the window
    [InlineData(0, 90, "ok")]
    [InlineData(75, 90, "ok")]
    // the due window opens at maxAge - 14
    [InlineData(76, 90, "due")]
    [InlineData(90, 90, "due")]
    // past the deadline
    [InlineData(91, 90, "overdue")]
    [InlineData(4000, 90, "overdue")]
    public void Status_is_computed_from_age_and_policy(int? age, int maxAge, string expected)
        => Assert.Equal(expected, SecretsEndpoints.StatusFor(age, maxAge));

    [Fact]
    public void The_due_window_is_exactly_fourteen_days_wide()
    {
        const int max = 180;
        Assert.Equal("ok", SecretsEndpoints.StatusFor(max - SecretsEndpoints.DueWindowDays - 1, max));
        Assert.Equal("due", SecretsEndpoints.StatusFor(max - SecretsEndpoints.DueWindowDays, max));
    }

    [Fact]
    public void Every_inventory_entry_is_complete_and_uniquely_keyed()
    {
        Assert.NotEmpty(SecretsEndpoints.Inventory);
        Assert.Equal(
            SecretsEndpoints.Inventory.Select(s => s.Key).Distinct(StringComparer.Ordinal).Count(),
            SecretsEndpoints.Inventory.Count);

        Assert.All(SecretsEndpoints.Inventory, s =>
        {
            Assert.False(string.IsNullOrWhiteSpace(s.Key));
            Assert.False(string.IsNullOrWhiteSpace(s.Name));
            Assert.False(string.IsNullOrWhiteSpace(s.Description));
            Assert.True(s.MaxAgeDays > SecretsEndpoints.DueWindowDays,
                $"'{s.Key}' has a policy shorter than the due window, so it could never read 'ok'");
        });
    }

    [Fact]
    public void The_credential_key_is_marked_NOT_automated_and_says_why()
    {
        // If this ever flips to automated without a re-encryption step existing,
        // running the rotation makes every stored cloud credential permanently
        // unreadable. The test exists to make that flip impossible to do quietly.
        var key = SecretsEndpoints.Inventory.Single(s => s.Key == "credential-key");
        Assert.False(key.Automated);
        Assert.NotNull(key.Risk);
        Assert.Contains("re-encrypt", key.Risk!, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void The_inventory_carries_no_secret_material()
    {
        // Belt and braces: the inventory is compiled into the binary and shipped
        // to every admin browser, so it must describe secrets, never hold one.
        Assert.All(SecretsEndpoints.Inventory, s =>
        {
            var blob = string.Join(' ', s.Key, s.Name, s.Description, s.Risk ?? "");
            Assert.DoesNotContain("BEGIN PRIVATE KEY", blob, StringComparison.OrdinalIgnoreCase);
            Assert.DoesNotContain("ghp_", blob, StringComparison.Ordinal);
            Assert.DoesNotContain("Password=", blob, StringComparison.Ordinal);
        });
    }
}
