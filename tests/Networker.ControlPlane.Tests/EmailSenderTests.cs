using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Logging.Abstractions;
using Networker.ControlPlane.Notifications;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Tests for the email sender port of Rust <c>email.rs</c> — env-driven selection
/// (ACS vs no-op) and the no-op log fallback behavior.
/// </summary>
public sealed class EmailSenderTests
{
    private static IEmailSender Resolve(string? conn, string? sender)
    {
        var prevConn = Environment.GetEnvironmentVariable(EmailEnv.AcsConnectionString);
        var prevSender = Environment.GetEnvironmentVariable(EmailEnv.AcsSender);
        try
        {
            Environment.SetEnvironmentVariable(EmailEnv.AcsConnectionString, conn);
            Environment.SetEnvironmentVariable(EmailEnv.AcsSender, sender);

            var services = new ServiceCollection();
            services.AddLogging(b => b.SetMinimumLevel(LogLevel.Warning));
            services.AddNetworkerEmailSender();
            return services.BuildServiceProvider().GetRequiredService<IEmailSender>();
        }
        finally
        {
            Environment.SetEnvironmentVariable(EmailEnv.AcsConnectionString, prevConn);
            Environment.SetEnvironmentVariable(EmailEnv.AcsSender, prevSender);
        }
    }

    [Fact]
    public void Selects_noop_when_unconfigured()
    {
        Assert.IsType<NoOpEmailSender>(Resolve(null, null));
        Assert.IsType<NoOpEmailSender>(Resolve("endpoint=https://x.communication.azure.com/;accesskey=AAAA", null));
        Assert.IsType<NoOpEmailSender>(Resolve(null, "no-reply@x.com"));
    }

    [Fact]
    public void Selects_acs_when_both_set()
    {
        var sender = Resolve(
            "endpoint=https://x.communication.azure.com/;accesskey=" + Convert.ToBase64String(new byte[] { 1, 2, 3 }),
            "no-reply@x.com");
        Assert.IsType<AcsEmailSender>(sender);
    }

    [Fact]
    public async Task NoOp_logs_and_returns_true()
    {
        var sender = new NoOpEmailSender(NullLogger<NoOpEmailSender>.Instance);
        var ok = await sender.SendAsync("to@x.com", "subject", "body");
        Assert.True(ok);
    }
}
