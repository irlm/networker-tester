using System.Globalization;
using System.Text;
using System.Text.Json;
using Networker.ControlPlane.Notifications;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Alerting;

/// <summary>
/// Delivers one <see cref="AlertNotification"/> through one
/// <see cref="AlertChannel"/> and reports the outcome as the
/// <c>alert_event.delivery_status</c> string (<c>delivered</c> /
/// <c>failed: ...</c>). Implementations must never throw for an ordinary
/// delivery problem — alerting is best-effort and must not fail run
/// processing (same contract as <see cref="IEmailSender"/>).
/// </summary>
public interface IAlertNotifier
{
    Task<string> DeliverAsync(AlertChannel channel, AlertNotification notification, CancellationToken ct = default);
}

/// <summary>
/// The real deliverer: webhook = POST the payload JSON with an optional
/// HMAC-SHA256 signature header (10s timeout, one retry); email = the
/// existing <see cref="IEmailSender"/> (ACS when configured, log-only no-op
/// otherwise), one send per configured recipient.
/// </summary>
public sealed class AlertNotifier(
    IHttpClientFactory httpFactory,
    IEmailSender emailSender,
    ILogger<AlertNotifier> logger) : IAlertNotifier
{
    /// <summary>Named HttpClient with the 10s webhook timeout (registered by
    /// <see cref="AlertingServiceCollectionExtensions.AddNetworkerAlerting"/>).</summary>
    public const string WebhookClientName = "alert-webhook";

    public const string StatusDelivered = "delivered";

    /// <summary>Attempts per webhook delivery (1 initial + 1 retry).</summary>
    private const int WebhookAttempts = 2;

    public async Task<string> DeliverAsync(
        AlertChannel channel, AlertNotification notification, CancellationToken ct = default)
    {
        try
        {
            return channel.Kind switch
            {
                "webhook" => await DeliverWebhookAsync(channel, notification, ct),
                "email" => await DeliverEmailAsync(channel, notification, ct),
                _ => $"failed: unknown channel kind '{channel.Kind}'",
            };
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            throw;
        }
        catch (Exception ex)
        {
            // Belt-and-braces: no delivery problem may escape into run processing.
            logger.LogWarning(ex, "Alert delivery failed on channel {ChannelId}", channel.ChannelId);
            return $"failed: {ex.GetType().Name}";
        }
    }

    private async Task<string> DeliverWebhookAsync(
        AlertChannel channel, AlertNotification notification, CancellationToken ct)
    {
        string? url = null;
        string? secret = null;
        try
        {
            using var doc = JsonDocument.Parse(channel.Config);
            if (doc.RootElement.TryGetProperty("url", out var u) && u.ValueKind == JsonValueKind.String)
            {
                url = u.GetString();
            }
            if (doc.RootElement.TryGetProperty("secret", out var s) && s.ValueKind == JsonValueKind.String)
            {
                secret = s.GetString();
            }
        }
        catch (JsonException)
        {
            return "failed: invalid channel config";
        }

        if (string.IsNullOrWhiteSpace(url) || !Uri.TryCreate(url, UriKind.Absolute, out var uri)
            || (uri.Scheme != Uri.UriSchemeHttp && uri.Scheme != Uri.UriSchemeHttps))
        {
            return "failed: invalid channel config";
        }

        // Serialize ONCE — the signature is computed over the exact bytes sent,
        // so receivers can verify against the raw request body.
        var payload = AlertWebhook.BuildPayloadJson(notification);

        var client = httpFactory.CreateClient(WebhookClientName);
        string lastFailure = "failed: unreachable";

        for (var attempt = 1; attempt <= WebhookAttempts; attempt++)
        {
            try
            {
                using var request = new HttpRequestMessage(HttpMethod.Post, uri)
                {
                    Content = new StringContent(payload, Encoding.UTF8, "application/json"),
                };
                if (!string.IsNullOrEmpty(secret))
                {
                    request.Headers.TryAddWithoutValidation(
                        AlertWebhook.SignatureHeader,
                        AlertWebhook.SignatureHeaderValue(payload, secret));
                }

                using var response = await client.SendAsync(request, ct);
                if (response.IsSuccessStatusCode)
                {
                    return StatusDelivered;
                }

                lastFailure = $"failed: http {(int)response.StatusCode}";
            }
            catch (OperationCanceledException) when (ct.IsCancellationRequested)
            {
                throw;
            }
            catch (OperationCanceledException)
            {
                lastFailure = "failed: timeout"; // the 10s HttpClient timeout
            }
            catch (HttpRequestException ex)
            {
                lastFailure = $"failed: {ex.HttpRequestError.ToString().ToLowerInvariant()}";
            }
        }

        logger.LogWarning(
            "Alert webhook delivery failed on channel {ChannelId}: {Status}",
            channel.ChannelId, lastFailure);
        return lastFailure;
    }

    private async Task<string> DeliverEmailAsync(
        AlertChannel channel, AlertNotification notification, CancellationToken ct)
    {
        List<string> recipients = [];
        try
        {
            using var doc = JsonDocument.Parse(channel.Config);
            if (doc.RootElement.TryGetProperty("to", out var to) && to.ValueKind == JsonValueKind.Array)
            {
                recipients = to.EnumerateArray()
                    .Where(e => e.ValueKind == JsonValueKind.String)
                    .Select(e => e.GetString()!)
                    .Where(a => !string.IsNullOrWhiteSpace(a))
                    .ToList();
            }
        }
        catch (JsonException)
        {
            return "failed: invalid channel config";
        }

        if (recipients.Count == 0)
        {
            return "failed: invalid channel config";
        }

        var subject = $"[networker] {notification.state}: {notification.metric} " +
                      $"{notification.comparator} {notification.threshold.ToString("0.###", CultureInfo.InvariantCulture)}";
        var body =
            $"{notification.message}\n\n" +
            $"state:       {notification.state}\n" +
            $"metric:      {notification.metric}\n" +
            $"value:       {(notification.value is { } v ? v.ToString("0.###", CultureInfo.InvariantCulture) : "n/a")}\n" +
            $"threshold:   {notification.comparator} {notification.threshold.ToString("0.###", CultureInfo.InvariantCulture)}\n" +
            $"project:     {notification.project_id}\n" +
            $"test config: {notification.test_config_id?.ToString() ?? "(all)"}\n" +
            $"run:         {notification.run_id}\n" +
            $"fired at:    {notification.fired_at:O}\n";

        var allSent = true;
        foreach (var recipient in recipients)
        {
            allSent &= await emailSender.SendAsync(recipient, subject, body, ct);
        }

        return allSent ? StatusDelivered : "failed: email send";
    }
}
