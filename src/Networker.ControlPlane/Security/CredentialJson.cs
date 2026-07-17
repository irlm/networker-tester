using System.Text.Json;

namespace Networker.ControlPlane.Security;

/// <summary>
/// The one place that flattens a decrypted cloud-credential JSON object into a
/// <c>string → string</c> map (string values as-is, non-strings as their raw
/// JSON text so nothing is silently dropped; non-object roots yield an empty
/// map). Previously four byte-identical copies lived in
/// <c>TesterPrecheckEndpoints</c>, <c>CloudAccountsEndpoints</c>,
/// <c>OrphanReaperService</c>, and <c>TesterWriteEndpoints.Create</c>.
/// </summary>
public static class CredentialJson
{
    /// <summary>
    /// Parse a decrypted credential blob. Invalid JSON throws
    /// <see cref="JsonException"/> — callers that must soft-fail (precheck's
    /// <c>decrypt_failed</c>, the reaper's skip-account path) already wrap the
    /// decrypt+parse pair in a try/catch.
    /// </summary>
    public static Dictionary<string, string> ToMap(byte[] plaintextJson)
    {
        using var doc = JsonDocument.Parse(plaintextJson);
        return Flatten(doc);
    }

    /// <summary>
    /// Lenient variant for the cloud-init credential path: invalid JSON yields
    /// an empty map instead of throwing — callers treat missing keys as absent
    /// config, matching the Rust serde behaviour.
    /// </summary>
    public static Dictionary<string, string> ToMapLenient(string json)
    {
        try
        {
            using var doc = JsonDocument.Parse(json);
            return Flatten(doc);
        }
        catch (JsonException)
        {
            return new Dictionary<string, string>(StringComparer.Ordinal);
        }
    }

    private static Dictionary<string, string> Flatten(JsonDocument doc)
    {
        var map = new Dictionary<string, string>(StringComparer.Ordinal);
        if (doc.RootElement.ValueKind == JsonValueKind.Object)
        {
            foreach (var prop in doc.RootElement.EnumerateObject())
            {
                map[prop.Name] = prop.Value.ValueKind == JsonValueKind.String
                    ? prop.Value.GetString() ?? string.Empty
                    : prop.Value.GetRawText();
            }
        }

        return map;
    }
}
