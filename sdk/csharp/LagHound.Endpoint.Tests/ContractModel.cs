using System.Text.Json;

namespace LagHound.Endpoint.Tests;

/// <summary>
/// Thin reader over shared/sdk-contract-v1.json. Rather than binding to a big
/// POCO, tests navigate the JSON document directly — the contract is the
/// source of truth and this keeps drift visible.
/// </summary>
internal sealed class ContractModel
{
    internal JsonDocument Doc { get; }
    internal JsonElement Root => Doc.RootElement;

    private ContractModel(JsonDocument doc) => Doc = doc;

    internal static ContractModel Load()
    {
        string path = Path.Combine(AppContext.BaseDirectory, "sdk-contract-v1.json");
        if (!File.Exists(path))
        {
            throw new FileNotFoundException(
                $"Conformance contract not found at {path}. It is copied from shared/sdk-contract-v1.json at build (see the .csproj <None> item).");
        }

        return new ContractModel(JsonDocument.Parse(File.ReadAllText(path)));
    }

    internal long Cap(string name) => Root.GetProperty("caps").GetProperty(name).GetInt64();

    internal JsonElement Route(string id)
    {
        foreach (var r in Root.GetProperty("routes").EnumerateArray())
        {
            if (r.GetProperty("id").GetString() == id)
            {
                return r;
            }
        }

        throw new KeyNotFoundException($"route '{id}' not in contract");
    }
}
