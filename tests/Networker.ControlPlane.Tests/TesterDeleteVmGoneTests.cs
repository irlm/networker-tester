using Networker.ControlPlane.Endpoints;
using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// <see cref="TesterWriteEndpoints.VmAlreadyGone"/> — the delete-path fix for the
/// "delete but doesn't delete" bug: a cloud delete that fails with "resource not
/// found" means the VM is already gone (desired state), so the tester row must be
/// removed, not kept forever for a retry that can never succeed.
/// </summary>
public sealed class TesterDeleteVmGoneTests
{
    private static ProvisionResult Failed(string stderr) => ProvisionResult.Failed(1, string.Empty, stderr);

    [Theory]
    [InlineData("The resource 'projects/x/zones/us-central1-a/instances/tester-y' was not found")] // gcp
    [InlineData("An error occurred (InvalidInstanceID.NotFound): The instance ID 'i-abc' does not exist")] // aws
    [InlineData("(ResourceNotFound) The Resource 'Microsoft.Compute/virtualMachines/tester-z' was not found")] // azure
    [InlineData("Resource group 'x' could not be found.")]
    [InlineData("instance no longer exists")]
    public void NotFound_signals_are_treated_as_already_gone(string stderr)
    {
        Assert.True(TesterWriteEndpoints.VmAlreadyGone(Failed(stderr)));
    }

    [Theory]
    [InlineData("AuthorizationFailed: The client does not have authorization to perform action 'delete'")]
    [InlineData("Timed out waiting for the operation to complete")]
    [InlineData("QuotaExceeded: operation limit reached")]
    [InlineData("connection refused")]
    public void Real_failures_are_not_treated_as_gone(string stderr)
    {
        // These must KEEP the row (a retryable failure) — not silently drop a
        // tester whose VM might still exist and bill.
        Assert.False(TesterWriteEndpoints.VmAlreadyGone(Failed(stderr)));
    }

    [Fact]
    public void A_successful_delete_is_not_flagged_as_already_gone()
    {
        Assert.False(TesterWriteEndpoints.VmAlreadyGone(ProvisionResult.Ok(0, string.Empty, string.Empty)));
    }
}
