using System.Text.Json;
using Microsoft.Extensions.Logging.Abstractions;
using Networker.Agent;

namespace Networker.Agent.Tests;

/// <summary>
/// Command verb dispatch — mirrors the Rust <c>commands::run_command</c> tests:
/// <c>health</c> returns ok with version/os/arch/uptime; unknown verbs return an
/// error result carrying "unknown verb".
/// </summary>
public class CommandHandlerTests
{
    private sealed class NullSink : RawWebSocketClient.IFrameSink
    {
        public bool TrySend(AgentMessage message) => true;
    }

    private static CommandMessage Command(string verb) => new(
        CommandId: Guid.NewGuid(),
        ConfigId: null,
        Token: "ignored",
        Verb: verb,
        Args: JsonDocument.Parse("{}").RootElement.Clone(),
        TimeoutSecs: 30);

    [Fact]
    public void Unknown_verb_returns_error_status()
    {
        var handler = new CommandHandler(NullLogger<CommandHandler>.Instance);
        var result = handler.Run(Command("no_such_verb"), new NullSink());

        Assert.Equal("error", result.Status);
        Assert.Null(result.Result);
        Assert.NotNull(result.Error);
        Assert.Contains("unknown verb", result.Error!);
    }

    [Fact]
    public void Health_returns_ok_with_version_os_arch_uptime()
    {
        var handler = new CommandHandler(NullLogger<CommandHandler>.Instance);
        var cmd = Command("health");
        var result = handler.Run(cmd, new NullSink());

        Assert.Equal(cmd.CommandId, result.CommandId);
        Assert.Equal("ok", result.Status);
        Assert.Null(result.Error);
        Assert.NotNull(result.Result);

        var body = result.Result!.Value;
        Assert.True(body.TryGetProperty("version", out _));
        Assert.True(body.TryGetProperty("os", out _));
        Assert.True(body.TryGetProperty("arch", out _));
        Assert.True(body.TryGetProperty("uptime_secs", out _));
        Assert.True(body.TryGetProperty("disk_free_mb", out var disk));
        Assert.Equal(JsonValueKind.Null, disk.ValueKind); // Rust stubs disk to None
    }
}
