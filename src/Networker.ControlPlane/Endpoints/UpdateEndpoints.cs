using Networker.ControlPlane.Auth;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// C# port of the Rust dashboard's <c>api/update.rs</c> self-update triggers.
/// Both routes require the global Admin role (Rust: <c>require_role(Admin)</c>);
/// mounted in <c>protected_flat</c>, so they use the <c>GlobalAdmin</c> policy.
///
/// <para>Routes:</para>
/// <list type="bullet">
///   <item><b>POST /api/update/tester</b> — trigger local tester binary update.</item>
///   <item><b>POST /api/update/dashboard</b> — trigger dashboard self-update.</item>
/// </list>
///
/// <para><b>HONEST 501 (fidelity audit F24):</b> the Rust handlers downloaded a
/// GitHub release, extracted the tarball, overwrote the running binary, and
/// <c>exec()</c>-restarted the process. On the tarball deployment model
/// self-update is the release pipeline's job (release.yml deploys, health-checks,
/// and rolls back atomically) — an in-process self-update would fight it. The
/// previous C# behaviour returned <c>200 {"status":"updating"}</c> and did
/// nothing, so operators watched an "update" that never happened. Both routes
/// now return <c>501 { "error": ... }</c> until/unless a tarball-aware
/// self-update is deliberately built.</para>
/// </summary>
public static class UpdateEndpoints
{
    private const string NotImplementedMessage =
        "self-update is not implemented in the C# control plane yet — nothing was updated; "
        + "tracked in the fidelity audit (F24). Updates ship via the release pipeline: "
        + "tagging a release deploys, health-checks, and auto-rolls-back (docs/release-flow.md).";

    public static IEndpointRouteBuilder MapUpdateEndpoints(this IEndpointRouteBuilder app)
    {
        // POST /api/update/tester — trigger local tester update (admin).
        app.MapPost("/api/update/tester", (
            HttpContext ctx,
            ILoggerFactory loggerFactory) =>
            Refuse(loggerFactory, "update/tester")).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // POST /api/update/dashboard — trigger dashboard self-update (admin).
        app.MapPost("/api/update/dashboard", (
            HttpContext ctx,
            ILoggerFactory loggerFactory) =>
            Refuse(loggerFactory, "update/dashboard")).RequireAuthorization(AuthPolicies.GlobalAdmin);

        return app;
    }

    private static IResult Refuse(ILoggerFactory loggerFactory, string route)
    {
        loggerFactory.CreateLogger("Networker.Update").LogWarning(
            "{Route} requested — refused with 501: self-update is the release pipeline's job on the tarball model",
            route);
        return ApiError.Status(StatusCodes.Status501NotImplemented, NotImplementedMessage);
    }
}
