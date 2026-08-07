using System.Text.Json;
using BenchmarkDotNet.Attributes;
using Networker.Contracts;

namespace Networker.Benchmarks;

/// <summary>
/// (De)serialization of the versioned tester JSON contract — the seam every
/// run result crosses on ingest. The fixture is the pinned golden contract
/// sample (tests/Networker.Tests/fixtures/tester-golden.json) with its attempt
/// replicated to a 50-attempt batch, i.e. one realistic run's payload. Uses
/// the same source-generated ProbeContractJsonContext as production ingest.
/// </summary>
[MemoryDiagnoser]
public class ContractsBenchmarks
{
    private byte[] _json = [];
    private ProbeRunResult _result = new();

    [GlobalSetup]
    public void Setup()
    {
        var goldenPath = Path.Combine(AppContext.BaseDirectory, "fixtures", "tester-golden.json");
        var golden = JsonSerializer.Deserialize(
                         File.ReadAllBytes(goldenPath),
                         ProbeContractJsonContext.Default.ProbeRunResult)
                     ?? throw new InvalidOperationException("golden fixture failed to deserialize");

        var template = golden.Attempts.Count > 0
            ? golden.Attempts[0]
            : throw new InvalidOperationException("golden fixture has no attempts");

        var attempts = new List<ProbeAttempt>(50);
        for (var i = 0; i < 50; i++)
        {
            attempts.Add(template with { SequenceNum = (uint)i, AttemptId = $"bench-{i:d4}" });
        }

        _result = golden with { Attempts = attempts };
        _json = JsonSerializer.SerializeToUtf8Bytes(
            _result, ProbeContractJsonContext.Default.ProbeRunResult);
    }

    [Benchmark]
    public ProbeRunResult? Deserialize() =>
        JsonSerializer.Deserialize(_json, ProbeContractJsonContext.Default.ProbeRunResult);

    [Benchmark]
    public byte[] Serialize() =>
        JsonSerializer.SerializeToUtf8Bytes(_result, ProbeContractJsonContext.Default.ProbeRunResult);
}
