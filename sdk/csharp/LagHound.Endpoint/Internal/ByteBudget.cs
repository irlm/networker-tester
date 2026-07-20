using System.Diagnostics;

namespace LagHound.Endpoint.Internal;

/// <summary>
/// Optional transfer byte budget per window (contract §6.4). Windowed
/// counter: once <c>consumed &gt;= budget</c> within the current window,
/// transfers get 429 with Retry-After set to the window remainder.
/// State is O(1) regardless of traffic.
/// </summary>
internal sealed class ByteBudget
{
    private readonly long _budgetBytes;
    private readonly int _windowSeconds;
    private readonly object _lock = new();
    private long _windowStart;
    private long _consumed;

    internal ByteBudget(long budgetBytes, int windowSeconds)
    {
        _budgetBytes = budgetBytes;
        _windowSeconds = windowSeconds;
        _windowStart = Stopwatch.GetTimestamp();
    }

    /// <summary>Reserve <paramref name="bytes"/> (used by /download where size is known upfront).</summary>
    internal bool TryReserve(long bytes, out int retryAfterSeconds)
    {
        lock (_lock)
        {
            RollWindow();
            if (_consumed >= _budgetBytes)
            {
                retryAfterSeconds = Remainder();
                return false;
            }

            _consumed += bytes;
            retryAfterSeconds = 0;
            return true;
        }
    }

    /// <summary>Exhaustion check without consuming (used by /upload before the drain).</summary>
    internal bool IsExhausted(out int retryAfterSeconds)
    {
        lock (_lock)
        {
            RollWindow();
            if (_consumed >= _budgetBytes)
            {
                retryAfterSeconds = Remainder();
                return true;
            }

            retryAfterSeconds = 0;
            return false;
        }
    }

    /// <summary>Record bytes after the fact (used by /upload once the drain count is known).</summary>
    internal void Record(long bytes)
    {
        lock (_lock)
        {
            RollWindow();
            _consumed += bytes;
        }
    }

    private void RollWindow()
    {
        if (Stopwatch.GetElapsedTime(_windowStart).TotalSeconds >= _windowSeconds)
        {
            _windowStart = Stopwatch.GetTimestamp();
            _consumed = 0;
        }
    }

    private int Remainder()
    {
        double left = _windowSeconds - Stopwatch.GetElapsedTime(_windowStart).TotalSeconds;
        return Math.Max(1, (int)Math.Ceiling(left));
    }
}
