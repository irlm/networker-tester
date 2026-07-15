namespace Networker.ControlPlane.Notifications;

/// <summary>
/// Transactional email sender — the C# port of the Rust dashboard mailer
/// (<c>crates/networker-dashboard/src/email.rs</c>, <c>send_email</c>).
///
/// <para>Every send is <b>best-effort</b>: the Rust call sites all invoke
/// <c>send_email</c> with <c>let _ = ...</c> / a WARN on error and NEVER fail the
/// surrounding operation (invite creation, password reset, workspace-inactivity
/// notice, …). Implementations here mirror that: <see cref="SendAsync"/> returns
/// a <see cref="bool"/> (true = sent / logged, false = a hard failure the caller
/// may log at WARN) and does not throw for a non-fatal delivery problem.</para>
///
/// <para><b>Env-driven implementation selection</b> matches Rust exactly: when
/// both <c>DASHBOARD_ACS_CONNECTION_STRING</c> and <c>DASHBOARD_ACS_SENDER</c>
/// are set, <see cref="AcsEmailSender"/> sends via Azure Communication Services;
/// otherwise <see cref="NoOpEmailSender"/> logs the message instead (the Rust
/// "ACS not configured — logging instead" dev fallback, and the same conceptual
/// role as the current M5 endpoint email log-stubs).</para>
/// </summary>
public interface IEmailSender
{
    /// <summary>
    /// Send a plain-text email. Faithful to Rust <c>send_email(to, subject,
    /// body)</c>: subject + plain-text body only (no HTML part), single "to"
    /// recipient. Never throws for an ordinary delivery failure — returns
    /// <c>false</c> so the caller can WARN and carry on, exactly like the Rust
    /// call sites.
    /// </summary>
    /// <returns><c>true</c> when the message was sent (or logged by the no-op
    /// fallback); <c>false</c> on a hard send failure.</returns>
    Task<bool> SendAsync(string to, string subject, string body, CancellationToken ct = default);
}

/// <summary>
/// Well-known environment variable names for the mailer — the exact strings the
/// Rust side reads (<c>email.rs</c> / <c>config.rs</c>). Kept in one place so the
/// sender + a later config-validation pass agree.
/// </summary>
public static class EmailEnv
{
    /// <summary>ACS connection string, format
    /// <c>endpoint=https://xxx.communication.azure.com/;accesskey=&lt;base64&gt;</c>.
    /// Rust: <c>DASHBOARD_ACS_CONNECTION_STRING</c>.</summary>
    public const string AcsConnectionString = "DASHBOARD_ACS_CONNECTION_STRING";

    /// <summary>Verified ACS sender ("from") address. Rust:
    /// <c>DASHBOARD_ACS_SENDER</c>.</summary>
    public const string AcsSender = "DASHBOARD_ACS_SENDER";
}
