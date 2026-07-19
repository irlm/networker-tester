namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// The one server-side verdict rule for a finished run — the C# twin of the
/// frontend's <c>runDisplayStatus()</c> (<c>dashboard/src/lib/runStatus.ts</c>,
/// audit F9), exposed so ALL API consumers share the same reading instead of
/// re-deriving it:
///
/// <list type="bullet">
///   <item>completed, zero failures → <c>completed</c></item>
///   <item>completed, some failures + some successes → <c>partial</c></item>
///   <item>completed, everything failed → <c>failed</c></item>
///   <item>anything else → the stored status verbatim</item>
/// </list>
///
/// It is emitted as a SEPARATE computed field (<c>result_status</c>) next to
/// the raw <c>status</c> on the v2 test-run responses; <c>status</c> itself is
/// never rewritten, so existing clients that key on the stored lifecycle
/// status (running/queued/completed/failed/cancelled) are unaffected, and the
/// frontend's own computation stays valid (it yields the identical value).
/// </summary>
public static class RunVerdict
{
    /// <summary>Compute <c>result_status</c> from the stored status + counters.</summary>
    public static string ResultStatus(string status, int successCount, int failureCount)
    {
        if (status == "completed" && failureCount > 0)
        {
            return successCount > 0 ? "partial" : "failed";
        }

        return status;
    }
}
