namespace Networker.ControlPlane.Notifications;

/// <summary>
/// Development / unconfigured-ACS fallback sender — the C# port of the Rust
/// <c>send_email</c> dev branch in <c>crates/networker-dashboard/src/email.rs</c>
/// ("ACS not configured — logging instead"). Used whenever
/// <c>DASHBOARD_ACS_CONNECTION_STRING</c> or <c>DASHBOARD_ACS_SENDER</c> is
/// missing.
///
/// <para>Instead of delivering, it logs the recipient, subject, and body at INFO
/// — the same behavior as the current M5 endpoint email log-stubs, now behind
/// the <see cref="IEmailSender"/> seam so a later wiring pass can inject the real
/// <see cref="AcsEmailSender"/> without touching the endpoints again.</para>
/// </summary>
public sealed class NoOpEmailSender : IEmailSender
{
    private readonly ILogger<NoOpEmailSender> _logger;

    public NoOpEmailSender(ILogger<NoOpEmailSender> logger) => _logger = logger;

    public Task<bool> SendAsync(string to, string subject, string body, CancellationToken ct = default)
    {
        // Rust: info!(to, subject, "EMAIL (ACS not configured — logging instead):\n{body}").
        _logger.LogInformation(
            "EMAIL (ACS not configured — logging instead) to={To} subject={Subject}\n{Body}",
            to, subject, body);
        return Task.FromResult(true);
    }
}
