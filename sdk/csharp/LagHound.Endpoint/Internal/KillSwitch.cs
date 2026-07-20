using System.Diagnostics;

namespace LagHound.Endpoint.Internal;

/// <summary>
/// <c>LAGHOUND_DISABLED=1</c> kill switch (contract §6.5). Evaluated at
/// request time with the env read cached for ≤ 1 second, so it can be
/// flipped live without a code deploy.
/// </summary>
internal static class KillSwitch
{
    private static readonly object Lock = new();
    private static long _nextCheck; // Stopwatch timestamp after which we re-read the env
    private static bool _disabled;

    internal static bool IsDisabled()
    {
        long now = Stopwatch.GetTimestamp();
        if (Volatile.Read(ref _nextCheck) <= now)
        {
            lock (Lock)
            {
                if (_nextCheck <= now)
                {
                    _disabled = Environment.GetEnvironmentVariable("LAGHOUND_DISABLED") == "1";
                    _nextCheck = now + Stopwatch.Frequency; // +1 s
                }
            }
        }

        return _disabled;
    }

    /// <summary>Test hook: force an immediate env re-read (the cache window is ≤ 1 s in production).</summary>
    internal static void Refresh()
    {
        lock (Lock)
        {
            _disabled = Environment.GetEnvironmentVariable("LAGHOUND_DISABLED") == "1";
            _nextCheck = Stopwatch.GetTimestamp() + Stopwatch.Frequency;
        }
    }
}
