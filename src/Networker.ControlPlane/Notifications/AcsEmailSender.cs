using Azure;
using Azure.Communication.Email;

namespace Networker.ControlPlane.Notifications;

/// <summary>
/// Azure Communication Services email sender — the C# port of the Rust
/// <c>send_via_acs</c> path in <c>crates/networker-dashboard/src/email.rs</c>.
///
/// <para>Rust hand-rolls the ACS REST call (HMAC-SHA256 request signing against
/// <c>/emails:send?api-version=2023-03-31</c>, base64(SHA-256(body)) content
/// hash, the <c>x-ms-date;host;x-ms-content-sha256</c> signed-headers scheme).
/// This C# port uses the official <b>Azure.Communication.Email</b> SDK
/// (<see cref="EmailClient"/>), which performs the identical connection-string
/// parse + request signing internally — same endpoint, same auth model, same
/// wire request — so we don't re-implement the crypto. The observable behavior
/// (send a plain-text email from the configured sender to a single recipient)
/// matches Rust.</para>
///
/// <para><b>Config</b> (read at construction, matching the Rust env var names):
/// <list type="bullet">
///   <item><c>DASHBOARD_ACS_CONNECTION_STRING</c> — the ACS connection string
///     (<c>endpoint=...;accesskey=...</c>), fed straight to
///     <see cref="EmailClient(string)"/> which parses it exactly as Rust's
///     <c>parse_acs_connection_string</c> does.</item>
///   <item><c>DASHBOARD_ACS_SENDER</c> — the verified "from" address
///     (<c>senderAddress</c> in the Rust JSON body).</item>
/// </list>
/// The env-driven <b>selection</b> (use ACS only when BOTH are present, else the
/// no-op logger) is done in <see cref="EmailSenderExtensions"/>, mirroring the
/// Rust branch in <c>send_email</c>.</para>
///
/// <para><b>Best-effort:</b> like the Rust call sites (all <c>let _ = ...</c>),
/// <see cref="SendAsync"/> swallows send failures and returns <c>false</c>
/// instead of throwing, so a mail outage never fails an invite / reset / notice.</para>
/// </summary>
public sealed class AcsEmailSender : IEmailSender
{
    private readonly EmailClient _client;
    private readonly string _sender;
    private readonly ILogger<AcsEmailSender> _logger;

    public AcsEmailSender(string connectionString, string sender, ILogger<AcsEmailSender> logger)
    {
        // EmailClient(connectionString) parses endpoint=...;accesskey=... exactly
        // like Rust's parse_acs_connection_string and signs requests with the same
        // HMAC-SHA256 scheme against the same api-version.
        _client = new EmailClient(connectionString);
        _sender = sender;
        _logger = logger;
    }

    // Test/DI seam: inject a pre-built client (e.g. a fake) instead of a
    // connection string.
    internal AcsEmailSender(EmailClient client, string sender, ILogger<AcsEmailSender> logger)
    {
        _client = client;
        _sender = sender;
        _logger = logger;
    }

    public async Task<bool> SendAsync(string to, string subject, string body, CancellationToken ct = default)
    {
        try
        {
            var content = new EmailContent(subject) { PlainText = body };
            var message = new EmailMessage(_sender, to, content);

            // WaitUntil.Started mirrors Rust's fire-and-forget POST (Rust does not
            // poll the ACS operation to completion — it treats an accepted 2xx/202
            // as success). Started returns as soon as ACS accepts the request.
            await _client
                .SendAsync(WaitUntil.Started, message, ct)
                .ConfigureAwait(false);

            // Rust logs INFO "Email sent via ACS".
            _logger.LogInformation("Email sent via ACS to {To} (subject: {Subject})", to, subject);
            return true;
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            throw;
        }
        catch (Exception ex)
        {
            // Rust bails "ACS email failed: HTTP {status} — {body}" and the caller
            // logs it at WARN without failing the operation. Same here.
            _logger.LogWarning(ex, "ACS email failed for {To} (subject: {Subject})", to, subject);
            return false;
        }
    }
}
