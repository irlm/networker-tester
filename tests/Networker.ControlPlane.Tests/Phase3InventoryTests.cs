using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Ports the Rust <c>api/inventory.rs</c> unit tests for the <c>is_managed</c>
/// exact/substring host-matching predicate.
/// </summary>
public class Phase3InventoryTests
{
    [Fact]
    public void ManagedByFqdnExactMatch()
    {
        var managed = new[] { "ec2-1-2-3-4.compute-1.amazonaws.com" };
        Assert.True(InventoryEndpoints.IsManaged("ec2-1-2-3-4.compute-1.amazonaws.com", null, managed));
    }

    [Fact]
    public void ManagedByIpExactMatch()
    {
        var managed = new[] { "10.0.0.5" };
        Assert.True(InventoryEndpoints.IsManaged(null, "10.0.0.5", managed));
    }

    [Fact]
    public void NotManagedWhenNoMatch()
    {
        var managed = new[] { "10.0.0.1" };
        Assert.False(InventoryEndpoints.IsManaged("other.host.com", "10.0.0.99", managed));
    }

    [Fact]
    public void NotManagedWhenEmptyList()
    {
        Assert.False(InventoryEndpoints.IsManaged("host.com", "1.2.3.4", Array.Empty<string>()));
    }

    [Fact]
    public void NotManagedWhenBothNull()
    {
        var managed = new[] { "10.0.0.1" };
        Assert.False(InventoryEndpoints.IsManaged(null, null, managed));
    }

    [Fact]
    public void ManagedByPartialIpInFqdn()
    {
        var managed = new[] { "ec2-10-0-0-5.compute.amazonaws.com" };
        Assert.True(InventoryEndpoints.IsManaged("ec2-10-0-0-5.compute.amazonaws.com", null, managed));
    }
}
