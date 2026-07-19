using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

namespace Networker.ControlPlane.Alerting;

/// <summary>
/// One notification, fully resolved — what a channel delivers. Snake_case
/// property names so the serialized webhook body matches the control plane's
/// wire convention (see <c>docs/alerting.md</c> for the payload contract).
/// </summary>
public sealed record AlertNotification(
    Guid event_id,
    Guid rule_id,
    string project_id,
    Guid? test_config_id,
    Guid run_id,
    string metric,
    string comparator,
    double threshold,
    double? value,
    string state,
    string message,
    DateTime fired_at);

/// <summary>
/// Webhook payload + signature construction — pure and unit-testable. The
/// payload is serialized ONCE and the exact bytes are both signed and sent,
/// so receivers can verify the signature over the raw request body.
/// </summary>
public static class AlertWebhook
{
    /// <summary>HTTP header carrying the HMAC signature (present only when the
    /// channel config has a <c>secret</c>).</summary>
    public const string SignatureHeader = "X-Networker-Signature";

    private static readonly JsonSerializerOptions PayloadOptions = new()
    {
        // Property names are already snake_case on AlertNotification; keep
        // them verbatim and ISO-8601 UTC timestamps (STJ default).
        WriteIndented = false,
    };

    /// <summary>Serialize the notification to the JSON body that is POSTed.</summary>
    public static string BuildPayloadJson(AlertNotification notification) =>
        JsonSerializer.Serialize(notification, PayloadOptions);

    /// <summary>
    /// <c>sha256=&lt;lowercase-hex HMAC-SHA256(secret, payload)&gt;</c> — the
    /// value of the <see cref="SignatureHeader"/> header. Key and payload are
    /// both UTF-8 encoded.
    /// </summary>
    public static string SignatureHeaderValue(string payloadJson, string secret)
    {
        var mac = HMACSHA256.HashData(
            Encoding.UTF8.GetBytes(secret),
            Encoding.UTF8.GetBytes(payloadJson));
        return $"sha256={Convert.ToHexStringLower(mac)}";
    }
}
