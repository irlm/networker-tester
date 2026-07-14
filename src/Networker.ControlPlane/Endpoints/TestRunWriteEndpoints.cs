using Networker.ControlPlane.Dispatch;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// REST v2 <b>write</b> endpoints for test runs — the C# port of the Rust
/// <c>crates/networker-dashboard/src/api/test_runs.rs</c> <c>cancel_handler</c>
/// (M1 ported only the reads). Delegates to the M3 <see cref="IRunDispatcher"/>,
/// which flips the run to <c>cancelled</c>, sends <c>CancelRun</c> to the owning
/// agent if online, and publishes a <c>JobUpdate</c>.
/// </summary>
public static class TestRunWriteEndpoints
{
    public static IEndpointRouteBuilder MapTestRunWriteEndpoints(this IEndpointRouteBuilder app)
    {
        // POST /api/v2/test-runs/{id}/cancel — cooperative cancel. Auth only (flat
        // route, no {projectId} for the project-scope policy — same follow-up
        // caveat as the M1 read side). Returns 202 Accepted; the DB status is set
        // synchronously, the agent-side cancel is best-effort.
        app.MapPost("/api/v2/test-runs/{id:guid}/cancel", async (
            Guid id,
            IRunDispatcher dispatcher,
            CancellationToken ct) =>
        {
            try
            {
                await dispatcher.CancelAsync(id, ct);
                return Results.Accepted(
                    $"/api/v2/test-runs/{id}",
                    new { id, status = "cancelled" });
            }
            catch (RunDispatchNotFoundException)
            {
                return Results.NotFound();
            }
        }).RequireAuthorization();

        return app;
    }
}
