using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Unit tests for <see cref="ProvisioningOrchestrator.WithInsecureWorkload"/>, the
/// second half of the E2E-pass 2026-07-28 P1-14 fix. When a provisioned
/// proxy-stack target is promoted Pending→Network, the dispatcher's
/// proxy-kind-only <c>insecure</c> injection no longer fires, so the promote must
/// persist <c>insecure: true</c> itself — otherwise the tester validates the
/// deployed target's self-signed cert and fails every attempt at TLS (confirmed
/// live: TCP ok, no TLS phase, 50/50 fail even with the CA:FALSE cert).
/// </summary>
public class PromoteInsecureWorkloadTests
{
    [Fact]
    public void Adds_insecure_true_to_a_workload_without_it()
    {
        var result = ProvisioningOrchestrator.WithInsecureWorkload(
            """{"runs":10,"modes":["http1","http2"],"timeout_ms":5000}""");

        using var doc = System.Text.Json.JsonDocument.Parse(result);
        Assert.True(doc.RootElement.GetProperty("insecure").GetBoolean());
        // existing fields preserved
        Assert.Equal(10, doc.RootElement.GetProperty("runs").GetInt32());
        Assert.Equal(2, doc.RootElement.GetProperty("modes").GetArrayLength());
    }

    [Fact]
    public void Overwrites_an_existing_false_insecure()
    {
        var result = ProvisioningOrchestrator.WithInsecureWorkload(
            """{"runs":1,"insecure":false}""");

        using var doc = System.Text.Json.JsonDocument.Parse(result);
        Assert.True(doc.RootElement.GetProperty("insecure").GetBoolean());
    }

    [Theory]
    [InlineData("not json")]
    [InlineData("[1,2,3]")]   // array, not an object
    [InlineData("42")]
    public void Leaves_non_object_or_unparseable_workload_unchanged(string workload)
        => Assert.Equal(workload, ProvisioningOrchestrator.WithInsecureWorkload(workload));
}
