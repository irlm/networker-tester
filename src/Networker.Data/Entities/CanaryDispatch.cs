using System;

namespace Networker.Data.Entities;

/// <summary>
/// One in-product trigger of the prod run-execution canary
/// (<c>.github/workflows/soak-canary.yml</c>), persisted so the history
/// survives GitHub being unreachable and is visible from any deployment that
/// shares this database — not only from the GitHub Actions UI.
///
/// <para>A row is written the moment a dispatch is accepted (GitHub answers
/// 204 with no run id), capturing <b>who</b> triggered it, <b>when</b>, against
/// which ref, and with which inputs. The GitHub run id / status / conclusion
/// are filled in later by <c>CanaryRunPoller</c> once the run appears and
/// completes — the columns are nullable until then.</para>
/// </summary>
public partial class CanaryDispatch
{
    public Guid Id { get; set; }

    /// <summary>Email of the platform admin who triggered it (null if unknown).</summary>
    public string? RequestedBy { get; set; }

    public DateTime RequestedAt { get; set; }

    /// <summary>Git ref the canary ran against (branch/tag), e.g. <c>main</c>.</summary>
    public string GitRef { get; set; } = null!;

    /// <summary>The workflow_dispatch inputs as sent, JSON object
    /// (<c>{"apibench":"1",...}</c>). Stored as jsonb.</summary>
    public string Inputs { get; set; } = "{}";

    /// <summary>GitHub Actions run id once resolved by the poller (null until then).</summary>
    public long? RunId { get; set; }

    /// <summary>Web URL of the resolved run (null until resolved).</summary>
    public string? RunUrl { get; set; }

    /// <summary>GitHub run <c>status</c>: queued / in_progress / completed (null until resolved).</summary>
    public string? RunStatus { get; set; }

    /// <summary>GitHub run <c>conclusion</c>: success / failure / cancelled / … (null until completed).</summary>
    public string? Conclusion { get; set; }

    /// <summary>Last time the poller touched this row.</summary>
    public DateTime UpdatedAt { get; set; }
}
