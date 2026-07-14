using System.Collections.Concurrent;

namespace Networker.ControlPlane.Realtime;

/// <summary>
/// In-process registry backing the tester-queue hub (<c>/ws/testers</c>).
///
/// It is the C# analogue of the Rust <c>TesterQueueHub</c> service state
/// (crates/networker-dashboard/src/services/tester_queue_hub.rs): it tracks
/// which SignalR connections are subscribed to which <c>(projectId, testerId)</c>
/// pair, holds a per-tester monotonically increasing <c>seq</c>, and enforces
/// the per-project subscription cap. In the Rust build each subscriber owns an
/// mpsc channel and the hub fans messages out over those channels; here SignalR
/// Groups do the fan-out, so this registry maps each <c>(project, tester)</c> to
/// a stable group name and only has to bookkeep membership + counts + seq.
///
/// Registered as a singleton (see <see cref="TesterQueueHubExtensions"/>).
/// All state is stored in concurrent collections and mutated under a per-key
/// lock, so it is safe to call from concurrent hub invocations.
/// </summary>
public sealed class TesterQueueRegistry
{
    /// <summary>
    /// Default per-project subscription cap. Mirrors the Rust hub's
    /// <c>MAX_SUBS_PER_PROJECT</c> (~50). Overridable via
    /// <c>DASHBOARD_MAX_SUBS_PER_PROJECT</c>.
    /// </summary>
    public const int DefaultMaxSubsPerProject = 50;

    private readonly int _maxSubsPerProject;

    // (projectId, testerId) -> set of subscribed SignalR connection ids.
    private readonly ConcurrentDictionary<(string ProjectId, string TesterId), ConcurrentDictionary<string, byte>> _subs
        = new();

    // projectId -> distinct (project, tester) subscription count for the cap.
    private readonly ConcurrentDictionary<string, int> _projectSubCounts = new();

    // (projectId, testerId) -> monotonic seq counter (bumped via Interlocked).
    private readonly ConcurrentDictionary<(string ProjectId, string TesterId), long> _seqs = new();

    public TesterQueueRegistry()
    {
        var raw = Environment.GetEnvironmentVariable("DASHBOARD_MAX_SUBS_PER_PROJECT");
        _maxSubsPerProject = int.TryParse(raw, out var v) && v > 0 ? v : DefaultMaxSubsPerProject;
    }

    /// <summary>Stable SignalR group name for a (project, tester) pair.</summary>
    public static string GroupName(string projectId, string testerId) =>
        $"tq:{projectId}:{testerId}";

    /// <summary>
    /// Next seq for a tester. Monotonic and thread-safe (the Rust hub keeps a
    /// per-tester counter and increments it before every snapshot/update). The
    /// first value returned for a tester is 1.
    /// </summary>
    public ulong NextSeq(string projectId, string testerId)
    {
        var key = (projectId, testerId);
        var next = _seqs.AddOrUpdate(key, 1, static (_, cur) => cur + 1);
        return (ulong)next;
    }

    /// <summary>
    /// Current per-project distinct subscription count (each unique
    /// <c>(project, tester)</c> a connection subscribes to counts once).
    /// </summary>
    public int ProjectSubCount(string projectId) =>
        _projectSubCounts.TryGetValue(projectId, out var n) ? n : 0;

    /// <summary>
    /// Register <paramref name="connectionId"/> as a subscriber of
    /// <c>(projectId, testerId)</c>. Returns <c>true</c> if the subscription was
    /// added (or already present), <c>false</c> if adding a *new*
    /// <c>(project, tester)</c> pair would exceed the per-project cap.
    ///
    /// The cap counts distinct <c>(project, tester)</c> pairs with at least one
    /// subscriber, matching the Rust hub's per-project subscription accounting.
    /// </summary>
    public bool TrySubscribe(string projectId, string testerId, string connectionId)
    {
        var key = (projectId, testerId);

        // Fast path: pair already has subscribers → just add the connection.
        if (_subs.TryGetValue(key, out var existing))
        {
            existing.TryAdd(connectionId, 0);
            return true;
        }

        // Slow path: creating a new pair — enforce the per-project cap first.
        // Lock on the per-project counter object to make check-then-increment
        // atomic against concurrent first-subscribers for the same project.
        lock (ProjectLock(projectId))
        {
            // Re-check under lock (another thread may have created it).
            if (_subs.TryGetValue(key, out existing))
            {
                existing.TryAdd(connectionId, 0);
                return true;
            }

            var current = ProjectSubCount(projectId);
            if (current >= _maxSubsPerProject)
            {
                return false;
            }

            var set = new ConcurrentDictionary<string, byte>();
            set.TryAdd(connectionId, 0);
            _subs[key] = set;
            _projectSubCounts[projectId] = current + 1;
            return true;
        }
    }

    /// <summary>
    /// Remove <paramref name="connectionId"/> from a single
    /// <c>(projectId, testerId)</c> subscription. When the last subscriber of a
    /// pair leaves, the pair is dropped and the per-project count decremented.
    /// </summary>
    public void Unsubscribe(string projectId, string testerId, string connectionId)
    {
        var key = (projectId, testerId);
        if (!_subs.TryGetValue(key, out var set))
        {
            return;
        }

        set.TryRemove(connectionId, out _);
        if (!set.IsEmpty)
        {
            return;
        }

        // Last subscriber gone — retire the pair under the project lock so the
        // count stays consistent with TrySubscribe's create path.
        lock (ProjectLock(projectId))
        {
            if (_subs.TryGetValue(key, out set) && set.IsEmpty)
            {
                _subs.TryRemove(key, out _);
                var current = ProjectSubCount(projectId) - 1;
                if (current <= 0)
                {
                    _projectSubCounts.TryRemove(projectId, out _);
                }
                else
                {
                    _projectSubCounts[projectId] = current;
                }
            }
        }
    }

    /// <summary>
    /// Drop <paramref name="connectionId"/> from every <c>(project, tester)</c>
    /// pair whose tester matches <paramref name="testerId"/>, across all projects
    /// the connection is subscribed under. For each pair actually removed,
    /// <paramref name="onRemoved"/> is invoked with the resolved
    /// <c>(projectId, testerId)</c> so the caller can leave the SignalR group.
    ///
    /// The Rust inbound <c>unsubscribe_tester_queue</c> message carries tester
    /// ids only (no project), so this resolves the owning project(s) from the
    /// registry — matching the Rust handler that filters its per-connection
    /// subscription map by tester id.
    /// </summary>
    public void RemoveConnectionFromTester(
        string testerId, string connectionId, Func<string, string, Task> onRemoved)
    {
        foreach (var kvp in _subs)
        {
            if (kvp.Key.TesterId != testerId)
            {
                continue;
            }

            if (kvp.Value.ContainsKey(connectionId))
            {
                var projectId = kvp.Key.ProjectId;
                Unsubscribe(projectId, testerId, connectionId);
                // Fire-and-forget the group-leave; SignalR group ops are safe to
                // run without awaiting here and the registry is already updated.
                _ = onRemoved(projectId, testerId);
            }
        }
    }

    /// <summary>
    /// Drop a connection from every subscription it holds (call on disconnect).
    /// Mirrors the Rust hub's cleanup that runs when the socket closes.
    /// </summary>
    public void RemoveConnection(string connectionId)
    {
        foreach (var kvp in _subs)
        {
            var (projectId, testerId) = kvp.Key;
            if (kvp.Value.ContainsKey(connectionId))
            {
                Unsubscribe(projectId, testerId, connectionId);
            }
        }
    }

    /// <summary>Whether any connection is subscribed to this tester.</summary>
    public bool HasSubscribers(string projectId, string testerId) =>
        _subs.TryGetValue((projectId, testerId), out var set) && !set.IsEmpty;

    // Per-project lock objects for atomic cap check-then-mutate.
    private readonly ConcurrentDictionary<string, object> _projectLocks = new();

    private object ProjectLock(string projectId) =>
        _projectLocks.GetOrAdd(projectId, static _ => new object());
}
