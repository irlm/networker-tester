using System.Diagnostics;

namespace Networker.Endpoint;

/// <summary>
/// Shared state threaded through the endpoint handlers, mirroring the Rust
/// <c>AppState</c> struct. Registered as a singleton in DI.
/// </summary>
public sealed class AppState
{
    /// <summary>When set, every response gets an <c>Alt-Svc</c> H3 advert.</summary>
    public ushort? H3Port { get; init; }

    public ushort HttpPort { get; init; }
    public ushort HttpsPort { get; init; }
    public ushort UdpPort { get; init; }
    public ushort UdpThroughputPort { get; init; }

    public long StartedAtTicks { get; } = Stopwatch.GetTimestamp();
    public required SystemMeta SystemMeta { get; init; }

    /// <summary>Uptime in whole seconds since process start.</summary>
    public ulong UptimeSecs()
    {
        var elapsed = Stopwatch.GetElapsedTime(StartedAtTicks);
        return (ulong)elapsed.TotalSeconds;
    }
}
