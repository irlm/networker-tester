namespace Networker.ControlPlane.Observability;

/// <summary>
/// Live counters for <c>GET /api/logs/pipeline-status</c>. Until now that
/// endpoint returned hard-coded zeros with a <c>TODO(phase3)</c>, because there
/// was no pipeline to report on. There is one now, so it reports the real
/// thing.
///
/// <para>All counters are <see cref="Interlocked"/>-updated: they are written
/// from the flush loop and read from request threads, and a torn read here
/// would be a lie on an operator's dashboard.</para>
/// </summary>
public sealed class ServiceLogMetrics
{
    private long _written;
    private long _dropped;
    private long _flushes;
    private long _flushErrors;
    private long _lastFlushMs;
    private long _queueDepth;

    /// <summary>Rows successfully committed.</summary>
    public long EntriesWritten => Interlocked.Read(ref _written);

    /// <summary>Entries the bounded queue refused — the price of never blocking
    /// a request thread on the database. A non-zero value here means the sink
    /// is behind, not that logging is broken.</summary>
    public long EntriesDropped => Interlocked.Read(ref _dropped);

    public long FlushCount => Interlocked.Read(ref _flushes);

    public long FlushErrors => Interlocked.Read(ref _flushErrors);

    /// <summary>Duration of the most recent flush, milliseconds.</summary>
    public long LastFlushMs => Interlocked.Read(ref _lastFlushMs);

    /// <summary>Entries queued and not yet written.</summary>
    public long QueueDepth => Interlocked.Read(ref _queueDepth);

    public void OnEnqueued() => Interlocked.Increment(ref _queueDepth);

    public void OnDropped() => Interlocked.Increment(ref _dropped);

    public void OnFlushed(int rows, long elapsedMs)
    {
        Interlocked.Add(ref _written, rows);
        Interlocked.Add(ref _queueDepth, -rows);
        Interlocked.Increment(ref _flushes);
        Interlocked.Exchange(ref _lastFlushMs, elapsedMs);
    }

    /// <summary>A batch that could not be written. The rows leave the queue
    /// either way — retrying forever would turn a database outage into an
    /// unbounded memory leak — so they are counted as dropped, not written.</summary>
    public void OnFlushFailed(int rows)
    {
        Interlocked.Add(ref _dropped, rows);
        Interlocked.Add(ref _queueDepth, -rows);
        Interlocked.Increment(ref _flushErrors);
    }
}
