namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// C# port of the Rust dashboard's <c>api/leaderboard.rs</c>. In v0.28.0 the old
/// <c>db::benchmarks</c> module was removed as part of the TestConfig unification,
/// so every leaderboard handler is a stub returning empty data until the
/// leaderboard is rebuilt on top of <c>test_run</c> + <c>benchmark_artifact</c>.
///
/// All three routes are PUBLIC (mounted in the Rust <c>public_router</c> — no auth).
/// The Rust <c>protected_router</c> registers no additional routes.
/// </summary>
public static class LeaderboardEndpoints
{
    public static IEndpointRouteBuilder MapLeaderboardEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/leaderboard — stub: []
        app.MapGet("/api/leaderboard", () => Results.Ok(Array.Empty<object>()));

        // GET /api/leaderboard/grouped — stub: { "groups": [] }
        app.MapGet("/api/leaderboard/grouped", () => Results.Ok(new { groups = Array.Empty<object>() }));

        // GET /api/leaderboard/runs — stub: []
        app.MapGet("/api/leaderboard/runs", () => Results.Ok(Array.Empty<object>()));

        return app;
    }
}
