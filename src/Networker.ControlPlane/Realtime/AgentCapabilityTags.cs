using System.Text.Json;
using System.Text.Json.Nodes;

namespace Networker.ControlPlane.Realtime;

/// <summary>
/// Where the runner tool inventory lives on the agent row: the JSONB
/// <c>agent.tags</c> column, under a <c>capabilities</c> key —
/// <c>{"capabilities":{"chrome":true,"tshark":false}}</c>. Additive: any other
/// members of the tags object are preserved; a non-object / unparseable tags
/// value is replaced by a fresh object. No migration needed (V002 created the
/// column; nothing else wrote it).
/// </summary>
public static class AgentCapabilityTags
{
    public const string Key = "capabilities";

    /// <summary>Merge <paramref name="caps"/> into the tags JSON text. Returns
    /// the SAME string instance when nothing changes, so callers can skip the
    /// row write by reference/value comparison.</summary>
    public static string Merge(string? tagsJson, AgentCapabilities caps)
    {
        JsonObject root;
        try
        {
            root = !string.IsNullOrWhiteSpace(tagsJson) && JsonNode.Parse(tagsJson) is JsonObject o
                ? o
                : new JsonObject();
        }
        catch (JsonException)
        {
            root = new JsonObject();
        }

        var existing = Read(tagsJson);
        if (existing is not null && existing == caps)
        {
            return tagsJson!;
        }

        root[Key] = new JsonObject
        {
            ["chrome"] = caps.Chrome,
            ["tshark"] = caps.Tshark,
        };
        return root.ToJsonString();
    }

    /// <summary>The stored inventory, or null when the tags carry none
    /// (pre-0.28.208 agent, or never heartbeated).</summary>
    public static AgentCapabilities? Read(string? tagsJson)
    {
        if (string.IsNullOrWhiteSpace(tagsJson))
        {
            return null;
        }
        try
        {
            using var doc = JsonDocument.Parse(tagsJson);
            if (doc.RootElement.ValueKind != JsonValueKind.Object
                || !doc.RootElement.TryGetProperty(Key, out var c)
                || c.ValueKind != JsonValueKind.Object)
            {
                return null;
            }
            bool Flag(string name) => c.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.True;
            return new AgentCapabilities(Flag("chrome"), Flag("tshark"));
        }
        catch (JsonException)
        {
            return null;
        }
    }
}
