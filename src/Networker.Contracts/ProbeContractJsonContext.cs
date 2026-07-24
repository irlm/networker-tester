using System.Text.Json;
using System.Text.Json.Serialization;

namespace Networker.Contracts;

/// <summary>
/// System.Text.Json source-generation context for the tester JSON contract.
/// Reflection-free (de)serialization keeps the agent trim/AOT friendly.
/// </summary>
[JsonSourceGenerationOptions(
    PropertyNamingPolicy = JsonKnownNamingPolicy.SnakeCaseLower,
    // The Rust output carries many fields the C# layer does not model yet;
    // ignore them rather than throwing so the contract can grow additively.
    ReadCommentHandling = JsonCommentHandling.Skip)]
[JsonSerializable(typeof(ProbeRunResult))]
[JsonSerializable(typeof(ProbeAttempt))]
[JsonSerializable(typeof(DnsPhase))]
[JsonSerializable(typeof(TcpPhase))]
[JsonSerializable(typeof(TlsPhase))]
[JsonSerializable(typeof(HttpPhase))]
[JsonSerializable(typeof(UdpPhase))]
[JsonSerializable(typeof(ServerTimingPhase))]
[JsonSerializable(typeof(RpmPhase))]
[JsonSerializable(typeof(PingPhase))]
[JsonSerializable(typeof(PathPhase))]
[JsonSerializable(typeof(PathHopEntry))]
[JsonSerializable(typeof(DualStackPhase))]
[JsonSerializable(typeof(DualStackLegPhase))]
[JsonSerializable(typeof(WebSocketPhase))]
[JsonSerializable(typeof(PmtudPhase))]
[JsonSerializable(typeof(NetworkContextInfo))]
[JsonSerializable(typeof(GeoInfo))]
[JsonSerializable(typeof(LoadSample))]
[JsonSerializable(typeof(ClockSync))]
public partial class ProbeContractJsonContext : JsonSerializerContext
{
}
