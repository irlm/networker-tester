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
/// <para>Both return <c>{ status: "updating", update_id: &lt;uuid&gt; }</c> and
/// kick off the update in the background.</para>
///
/// <para><b>Stub divergence (TODO(phase3)):</b> the actual binary self-update is
/// host-specific — the Rust handlers download a GitHub release, extract a tarball,
/// overwrite the running binary, fix perms, and <c>exec()</c>-restart the process,
/// streaming progress over the event bus. None of that is appropriate to run from
/// the C# ControlPlane in this phase (no in-process tester subprocess, no
/// exec-restart, CI has no release assets). The ENDPOINTS + response shape are
/// ported faithfully; the update action itself is a logged no-op. The
/// <c>update_id</c> is still a fresh UUID so the wire contract holds.</para>
/// </summary>
public static class UpdateEndpoints
{
    public static IEndpointRouteBuilder MapUpdateEndpoints(this IEndpointRouteBuilder app)
    {
        // POST /api/update/tester — trigger local tester update (admin).
        app.MapPost("/api/update/tester", (
            HttpContext ctx,
            ILoggerFactory loggerFactory) =>
        {
            var updateId = Guid.NewGuid();
            var log = loggerFactory.CreateLogger("Networker.Update");

            // TODO(phase3): perform the real host-side tester binary update +
            // subprocess restart. Stubbed — see class remarks.
            log.LogInformation(
                "update/tester requested (update_id={UpdateId}) — self-update is a phase-3 stub; no binary was changed",
                updateId);

            return Results.Ok(new
            {
                status = "updating",
                update_id = updateId.ToString(),
            });
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        // POST /api/update/dashboard — trigger dashboard self-update (admin).
        app.MapPost("/api/update/dashboard", (
            HttpContext ctx,
            ILoggerFactory loggerFactory) =>
        {
            var updateId = Guid.NewGuid();
            var log = loggerFactory.CreateLogger("Networker.Update");

            // TODO(phase3): perform the real host-side dashboard binary + frontend
            // + agent update and exec-restart. Stubbed — see class remarks.
            log.LogInformation(
                "update/dashboard requested (update_id={UpdateId}) — self-update is a phase-3 stub; no binary was changed",
                updateId);

            return Results.Ok(new
            {
                status = "updating",
                update_id = updateId.ToString(),
            });
        }).RequireAuthorization(AuthPolicies.GlobalAdmin);

        return app;
    }
}
