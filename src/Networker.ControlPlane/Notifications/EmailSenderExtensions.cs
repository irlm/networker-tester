using Microsoft.Extensions.DependencyInjection.Extensions;

namespace Networker.ControlPlane.Notifications;

/// <summary>
/// DI wiring for <see cref="IEmailSender"/>. Selects the implementation from the
/// environment exactly like the Rust <c>send_email</c> branch:
/// <see cref="AcsEmailSender"/> when BOTH
/// <c>DASHBOARD_ACS_CONNECTION_STRING</c> and <c>DASHBOARD_ACS_SENDER</c> are set
/// (non-empty), else <see cref="NoOpEmailSender"/> (log-only dev fallback).
///
/// <para>Add one line in <c>Program.cs</c> (before endpoint mapping):
/// <code>builder.Services.AddNetworkerEmailSender();</code>
/// Then a later pass can constructor/parameter-inject <see cref="IEmailSender"/>
/// into the endpoints that currently log-stub email (see the class remarks on
/// <see cref="IEmailSender"/>).</para>
/// </summary>
public static class EmailSenderExtensions
{
    /// <summary>
    /// Register <see cref="IEmailSender"/> as a singleton, choosing ACS vs no-op
    /// from the environment at registration time (matching Rust reading the env
    /// vars to decide the send path). <c>TryAdd</c> so a test host can register a
    /// fake first.
    /// </summary>
    public static IServiceCollection AddNetworkerEmailSender(this IServiceCollection services)
    {
        var conn = Environment.GetEnvironmentVariable(EmailEnv.AcsConnectionString);
        var sender = Environment.GetEnvironmentVariable(EmailEnv.AcsSender);

        var acsConfigured = !string.IsNullOrEmpty(conn) && !string.IsNullOrEmpty(sender);

        if (acsConfigured)
        {
            services.TryAddSingleton<IEmailSender>(sp => new AcsEmailSender(
                conn!,
                sender!,
                sp.GetRequiredService<ILogger<AcsEmailSender>>()));
        }
        else
        {
            services.TryAddSingleton<IEmailSender, NoOpEmailSender>();
        }

        return services;
    }
}
