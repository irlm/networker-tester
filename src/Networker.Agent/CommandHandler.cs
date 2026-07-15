using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text.Json;

namespace Networker.Agent;

/// <summary>
/// Runs typed command envelopes (verb + args) and streams <c>command_log</c> +
/// a terminal <c>command_result</c> — the C# port of the Rust
/// <c>commands::run_command</c> dispatcher (crates/networker-agent/src/commands/).
///
/// Verb dispatch mirrors Rust exactly:
///   * <c>health</c> → agent version, OS, arch, uptime, disk-free (best-effort).
///   * any other verb → error result with <c>"unknown verb: {verb}"</c>.
///
/// Token validation is intentionally NOT performed (faithful to Rust: the
/// dispatcher trusts the <c>token</c> field; WS channel auth gates the
/// connection itself). <c>duration_ms</c> wraps the whole dispatch.
/// </summary>
public sealed class CommandHandler(ILogger<CommandHandler> logger)
{
    private static readonly long ProcessStartTicks = Stopwatch.GetTimestamp();

    private static readonly string AgentVersion =
        typeof(CommandHandler).Assembly.GetName().Version?.ToString() ?? "0.0.0";

    /// <summary>Execute a command; returns the terminal <c>command_result</c>
    /// frame. Log lines (none for health today) stream via <paramref name="sink"/>.</summary>
    public CommandResultMessage Run(CommandMessage cmd, RawWebSocketClient.IFrameSink sink)
    {
        var start = Stopwatch.GetTimestamp();
        logger.LogInformation("Received command {CommandId} verb={Verb}", cmd.CommandId, cmd.Verb);

        JsonElement? result = null;
        string? error = null;
        try
        {
            result = cmd.Verb switch
            {
                "health" => RunHealth(cmd, sink),
                _ => throw new InvalidOperationException($"unknown verb: {cmd.Verb}"),
            };
        }
        catch (Exception ex)
        {
            error = ex.Message;
        }

        var durationMs = (ulong)Stopwatch.GetElapsedTime(start).TotalMilliseconds;

        return error is null
            ? new CommandResultMessage(cmd.CommandId, "ok", result, null, durationMs)
            : new CommandResultMessage(cmd.CommandId, "error", null, error, durationMs);
    }

    /// <summary>The <c>health</c> verb: version / os / arch / uptime / disk-free.
    /// Mirrors the Rust <c>commands::health::run</c> body (args ignored).</summary>
    private static JsonElement RunHealth(CommandMessage cmd, RawWebSocketClient.IFrameSink sink)
    {
        _ = cmd;
        _ = sink; // health has nothing to stream — channel kept for parity

        var os = OperatingSystem.IsWindows() ? "windows"
            : OperatingSystem.IsMacOS() ? "macos"
            : OperatingSystem.IsLinux() ? "linux"
            : RuntimeInformation.OSDescription;

        var arch = RuntimeInformation.ProcessArchitecture switch
        {
            Architecture.X64 => "x86_64",
            Architecture.X86 => "x86",
            Architecture.Arm64 => "aarch64",
            Architecture.Arm => "arm",
            _ => RuntimeInformation.ProcessArchitecture.ToString().ToLowerInvariant(),
        };

        var payload = new Dictionary<string, object?>
        {
            ["version"] = AgentVersion,
            ["os"] = os,
            ["arch"] = arch,
            ["uptime_secs"] = UptimeSecs(),
            ["disk_free_mb"] = DiskFreeMb(), // Rust stubs this to null
        };

        return JsonDocument.Parse(JsonSerializer.Serialize(payload)).RootElement.Clone();
    }

    /// <summary>Best-effort uptime in seconds — reads <c>/proc/uptime</c> on
    /// Linux (Rust parity), 0 elsewhere / on parse failure.</summary>
    private static ulong UptimeSecs()
    {
        try
        {
            if (OperatingSystem.IsLinux() && File.Exists("/proc/uptime"))
            {
                var contents = File.ReadAllText("/proc/uptime");
                var first = contents.Split(' ', StringSplitOptions.RemoveEmptyEntries).FirstOrDefault();
                if (double.TryParse(first, System.Globalization.CultureInfo.InvariantCulture, out var secs))
                    return (ulong)secs;
            }
        }
        catch
        {
            // Fall through to 0.
        }

        return 0;
    }

    /// <summary>Best-effort free-disk reporting — Rust returns <c>None</c>; we
    /// mirror that (null) to keep the wire shape identical.</summary>
    private static object? DiskFreeMb() => null;
}
