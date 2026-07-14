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
public partial class ProbeContractJsonContext : JsonSerializerContext
{
}
