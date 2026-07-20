namespace LagHound.Endpoint.Internal;

/// <summary>
/// Bounded in-flight counter (contract v1 §6.3). Non-blocking: a request that
/// cannot acquire a slot is rejected (429) rather than queued, so LagHound can
/// never grow a backlog on a struggling host.
/// </summary>
internal sealed class ConcurrencyGate
{
    private readonly int _max;
    private int _current;

    internal ConcurrencyGate(int max) => _max = max;

    /// <summary>Try to acquire a slot. Returns a disposable release token, or null when full.</summary>
    internal Lease? TryAcquire()
    {
        int taken = Interlocked.Increment(ref _current);
        if (taken > _max)
        {
            Interlocked.Decrement(ref _current);
            return null;
        }

        return new Lease(this);
    }

    private void Release() => Interlocked.Decrement(ref _current);

    internal readonly struct Lease : IDisposable
    {
        private readonly ConcurrencyGate _gate;

        internal Lease(ConcurrencyGate gate) => _gate = gate;

        public void Dispose() => _gate.Release();
    }
}
