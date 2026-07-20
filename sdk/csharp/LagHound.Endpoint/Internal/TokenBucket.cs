using System.Diagnostics;

namespace LagHound.Endpoint.Internal;

/// <summary>Classic token bucket. Thread-safe, O(1), no timers.</summary>
internal sealed class TokenBucket
{
    private readonly double _rps;
    private readonly double _burst;
    private readonly object _lock = new();
    private double _tokens;
    private long _lastRefill;

    internal TokenBucket(double rps, double burst)
    {
        _rps = rps;
        _burst = burst;
        _tokens = burst;
        _lastRefill = Stopwatch.GetTimestamp();
    }

    internal bool TryTake()
    {
        lock (_lock)
        {
            long now = Stopwatch.GetTimestamp();
            double elapsedSeconds = (now - _lastRefill) / (double)Stopwatch.Frequency;
            _lastRefill = now;
            _tokens = Math.Min(_burst, _tokens + elapsedSeconds * _rps);
            if (_tokens >= 1.0)
            {
                _tokens -= 1.0;
                return true;
            }

            return false;
        }
    }
}

/// <summary>
/// Per-IP token buckets with a hard entry cap + LRU eviction (contract §6.2)
/// so an address-spraying attacker cannot grow memory unboundedly.
/// </summary>
internal sealed class PerIpRateLimiter
{
    private readonly double _rps;
    private readonly double _burst;
    private readonly int _maxEntries;
    private readonly object _lock = new();
    private readonly Dictionary<string, (TokenBucket Bucket, LinkedListNode<string> Node)> _map = new();
    private readonly LinkedList<string> _lru = new();

    internal PerIpRateLimiter(double rps, double burst, int maxEntries)
    {
        _rps = rps;
        _burst = burst;
        _maxEntries = maxEntries;
    }

    internal bool TryTake(string ip)
    {
        lock (_lock)
        {
            if (_map.TryGetValue(ip, out var entry))
            {
                _lru.Remove(entry.Node);
                _lru.AddFirst(entry.Node);
                return entry.Bucket.TryTake();
            }

            if (_map.Count >= _maxEntries)
            {
                var evict = _lru.Last!;
                _map.Remove(evict.Value);
                _lru.RemoveLast();
            }

            var bucket = new TokenBucket(_rps, _burst);
            var node = _lru.AddFirst(ip);
            _map[ip] = (bucket, node);
            return bucket.TryTake();
        }
    }
}
