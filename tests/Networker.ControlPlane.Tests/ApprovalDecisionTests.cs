using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pure-logic tests for the command-approval decision mapping — the C# mirror
/// of the mapping inside Rust <c>db::command_approvals::decide</c>
/// (<c>if approved { "approved" } else { "denied" }</c>) and the
/// <c>DecideRequest</c> body semantics.
/// </summary>
public class ApprovalDecisionTests
{
    [Fact]
    public void Approved_maps_to_approved_status()
    {
        Assert.Equal("approved", ApprovalsEndpoints.DecisionStatus(true));
    }

    [Fact]
    public void Denied_maps_to_denied_status()
    {
        Assert.Equal("denied", ApprovalsEndpoints.DecisionStatus(false));
    }

    [Fact]
    public void Body_with_approved_true_resolves_true()
    {
        var body = new DecideApprovalRequest(Approved: true, Approve: null, Reason: null);
        Assert.True(body.EffectiveApproved());
    }

    [Fact]
    public void Body_with_approve_alias_resolves()
    {
        var body = new DecideApprovalRequest(Approved: null, Approve: false, Reason: "nope");
        Assert.False(body.EffectiveApproved());
    }

    [Fact]
    public void Approved_wins_over_approve_alias_when_both_present()
    {
        var body = new DecideApprovalRequest(Approved: true, Approve: false, Reason: null);
        Assert.True(body.EffectiveApproved());
    }

    [Fact]
    public void Body_without_either_flag_resolves_null()
    {
        // The endpoint maps this to 400 Bad Request.
        var body = new DecideApprovalRequest(Approved: null, Approve: null, Reason: null);
        Assert.Null(body.EffectiveApproved());
    }

    [Fact]
    public void Decide_body_deserialises_rust_shape()
    {
        // Exactly what the Rust DecideRequest accepts.
        var body = System.Text.Json.JsonSerializer.Deserialize<DecideApprovalRequest>(
            """{"approved": true, "reason": "looks safe"}""");
        Assert.NotNull(body);
        Assert.True(body!.EffectiveApproved());
        Assert.Equal("looks safe", body.Reason);
    }
}
